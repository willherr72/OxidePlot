<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { SeriesInfoEntry } from '../renderer.js';

  /** Array of series info objects from renderer.seriesInfo(). */
  export let series: SeriesInfoEntry[];

  const dispatch = createEventDispatcher<{ change: void }>();

  import type { Renderer } from '../renderer.js';
  export let renderer: Renderer;

  /**
   * Convert a [r, g, b, a] (0..1 floats) array to a CSS rgba() string.
   */
  function toCSS(color: [number, number, number, number]): string {
    const [r, g, b, a] = color;
    return `rgba(${r * 255 | 0}, ${g * 255 | 0}, ${b * 255 | 0}, ${a})`;
  }

  /** Convert a [r,g,b,a] (0..1) color to a #rrggbb hex string for <input type=color>. */
  function toHex(color: [number, number, number, number]): string {
    const h = (v: number) =>
      Math.round(Math.min(1, Math.max(0, v)) * 255).toString(16).padStart(2, '0');
    return `#${h(color[0])}${h(color[1])}${h(color[2])}`;
  }

  /** Apply a hex color picked from the swatch's color input to series `i`. */
  function changeColor(i: number, hex: string) {
    const r = parseInt(hex.slice(1, 3), 16) / 255;
    const g = parseInt(hex.slice(3, 5), 16) / 255;
    const b = parseInt(hex.slice(5, 7), 16) / 255;
    renderer.setSeriesColor(i, r, g, b);
    dispatch('change');
  }

  function toggleVisible(i: number, visible: boolean) {
    renderer.setSeriesVisible(i, visible);
    dispatch('change');
  }

  function remove(i: number) {
    renderer.removeSeries(i);
    dispatch('change');
  }

  function moveUp(i: number) {
    if (i === 0) return;
    renderer.moveSeries(i, i - 1);
    dispatch('change');
  }

  function moveDown(i: number) {
    if (i >= series.length - 1) return;
    renderer.moveSeries(i, i + 1);
    dispatch('change');
  }

  // ── fx picker state ────────────────────────────────────────────────────────

  /** Which row's fx picker is currently open (null = none). */
  let openFxIndex: number | null = null;

  /** Per-row picker state — keyed by row index. */
  let fxKind: string = 'moving_average';
  let fxWindow: number = 5;
  let fxMode: string = 'minmax';
  let fxMethod: string = 'linear';
  let fxPoints: number = 500;

  /** Toggle the fx picker for row i; clicking the open row closes it. */
  function toggleFx(i: number) {
    if (openFxIndex === i) {
      openFxIndex = null;
    } else {
      openFxIndex = i;
      // Reset picker state to defaults each time a row is opened.
      fxKind = 'moving_average';
      fxWindow = 5;
      fxMode = 'minmax';
      fxMethod = 'linear';
      fxPoints = 500;
    }
  }

  /** Apply the current picker selection as a transform on series i. */
  function applyFx(i: number) {
    let params: { window?: number; mode?: string; method?: string; points?: number } | null = null;
    if (fxKind === 'moving_average') {
      params = { window: fxWindow };
    } else if (fxKind === 'normalize') {
      params = { mode: fxMode };
    } else if (fxKind === 'resample') {
      params = { method: fxMethod, points: fxPoints };
    }
    try {
      renderer.addTransform(i, fxKind, params);
    } catch (e) {
      console.error('addTransform failed:', e);
    }
    openFxIndex = null;
    dispatch('change');
  }
</script>

<div class="series-panel">
  <div class="panel-header">Series</div>
  <ul class="series-list">
    {#each series as s, i}
      <li class="series-item">
        <div class="series-row" class:hidden={!s.visible}>
          <!-- Color swatch — click to pick a custom color -->
          <label class="swatch-label" title="Click to change color">
            <span class="swatch" style="background:{toCSS(s.color)}"></span>
            <input
              class="swatch-input"
              type="color"
              value={toHex(s.color)}
              on:input={(e) => changeColor(i, e.currentTarget.value)}
              aria-label="Change series color"
            />
          </label>
          <!-- Name -->
          <span class="series-name" title={s.name}>{s.name}</span>
          <!-- Controls -->
          <span class="controls">
            <button
              class="ctrl-btn fx-btn"
              class:fx-active={openFxIndex === i}
              title="Apply math transform"
              on:click={() => toggleFx(i)}
            >fx</button>
            <button
              class="ctrl-btn"
              title={s.visible ? 'Hide series' : 'Show series'}
              aria-label={s.visible ? 'Hide series' : 'Show series'}
              on:click={() => toggleVisible(i, !s.visible)}
            >
              {#if s.visible}
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
              {:else}
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
              {/if}
            </button>
            <button
              class="ctrl-btn"
              title="Move up (lower z-order)"
              aria-label="Move series up"
              disabled={i === 0}
              on:click={() => moveUp(i)}
            ><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="18 15 12 9 6 15"/></svg></button>
            <button
              class="ctrl-btn"
              title="Move down (higher z-order)"
              aria-label="Move series down"
              disabled={i === series.length - 1}
              on:click={() => moveDown(i)}
            ><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="6 9 12 15 18 9"/></svg></button>
            <button
              class="ctrl-btn remove-btn"
              title="Remove series"
              aria-label="Remove series"
              on:click={() => remove(i)}
            ><svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg></button>
          </span>
        </div>

        {#if openFxIndex === i}
          <div class="fx-picker">
            <label class="fx-label">
              Transform
              <select class="fx-select" bind:value={fxKind}>
                <option value="moving_average">Moving average</option>
                <option value="derivative">Derivative</option>
                <option value="integral">Integral</option>
                <option value="normalize">Normalize</option>
                <option value="resample">Resample</option>
                <option value="abs">Abs</option>
                <option value="log">Log</option>
                <option value="sqrt">Sqrt</option>
              </select>
            </label>

            {#if fxKind === 'moving_average'}
              <label class="fx-label">
                Window
                <input
                  class="fx-input"
                  type="number"
                  bind:value={fxWindow}
                  min="1"
                  step="1"
                />
              </label>
            {:else if fxKind === 'normalize'}
              <label class="fx-label">
                Mode
                <select class="fx-select" bind:value={fxMode}>
                  <option value="minmax">Min-max</option>
                  <option value="zscore">Z-score</option>
                </select>
              </label>
            {:else if fxKind === 'resample'}
              <label class="fx-label">
                Method
                <select class="fx-select" bind:value={fxMethod}>
                  <option value="linear">Linear</option>
                  <option value="nearest">Nearest</option>
                  <option value="cubic">Cubic spline</option>
                </select>
              </label>
              <label class="fx-label">
                Points
                <input
                  class="fx-input"
                  type="number"
                  bind:value={fxPoints}
                  min="2"
                  step="1"
                />
              </label>
            {/if}

            <button class="fx-apply-btn" on:click={() => applyFx(i)}>Apply</button>
          </div>
        {/if}
      </li>
    {/each}
  </ul>
</div>

<style>
  .series-panel {
    position: absolute;
    top: 8px;
    right: 8px;
    background: var(--panel-bg-alpha);
    border: 1px solid var(--border-mid);
    border-radius: var(--radius);
    min-width: 180px;
    max-width: 260px;
    max-height: 420px;
    display: flex;
    flex-direction: column;
    font-family: var(--font-ui);
    font-size: 0.78rem;
    color: var(--text-dim);
    box-shadow: var(--shadow-panel);
    z-index: 20;
    overflow: hidden;
    backdrop-filter: blur(4px);
  }

  .panel-header {
    padding: 6px 10px;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .series-list {
    list-style: none;
    margin: 0;
    padding: 4px 0;
    overflow-y: auto;
  }

  .series-item {
    display: flex;
    flex-direction: column;
  }

  .series-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    transition: background 0.12s;
  }

  .series-row:hover {
    background: var(--series-row-hover);
  }

  .series-row.hidden {
    opacity: 0.45;
  }

  .swatch-label {
    position: relative;
    display: inline-flex;
    flex-shrink: 0;
    line-height: 0;
    cursor: pointer;
  }
  /* Native color input overlaid invisibly on the swatch so a click opens the
     OS color picker. */
  .swatch-input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    padding: 0;
    border: none;
    opacity: 0;
    cursor: pointer;
  }
  .swatch {
    display: inline-block;
    width: 12px;
    height: 12px;
    border-radius: 3px;
    flex-shrink: 0;
    border: 1px solid var(--swatch-border);
    transition: transform 0.12s, box-shadow 0.12s;
  }
  .swatch-label:hover .swatch {
    transform: scale(1.12);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .series-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.78rem;
    color: var(--series-name-text);
  }

  .controls {
    display: flex;
    gap: 2px;
    flex-shrink: 0;
  }

  .ctrl-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    background: transparent;
    border: none;
    color: var(--series-ctrl-btn);
    cursor: pointer;
    padding: 0;
    border-radius: var(--radius-sm);
    transition: color 0.12s, background 0.12s;
  }
  .ctrl-btn svg {
    display: block;
  }

  .ctrl-btn:hover:not(:disabled) {
    background: var(--series-ctrl-hover-bg);
    color: var(--btn-hover-text);
  }

  .ctrl-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .remove-btn:hover:not(:disabled) {
    background: rgba(160, 40, 40, 0.5);
    color: #ff8888;
  }

  /* fx button */
  .fx-btn {
    width: auto;
    padding: 0 7px;
    font-family: var(--font-data);
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    border: 1px solid transparent;
    color: var(--text-muted);
  }

  .fx-btn:hover:not(:disabled) {
    background: var(--series-ctrl-hover-bg);
    color: var(--btn-hover-text);
    border-color: var(--border-mid);
  }

  .fx-btn.fx-active {
    background: var(--btn-active-bg);
    color: var(--btn-active-text);
    border-color: var(--btn-active-border);
  }

  /* fx picker (inline expanding block under the row) */
  .fx-picker {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px;
    background: var(--panel-bg);
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
  }

  .fx-label {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    font-size: 0.75rem;
    color: var(--settings-label);
  }

  .fx-select {
    background: var(--btn-bg);
    color: var(--text-dim);
    border: 1px solid var(--btn-border);
    border-radius: 3px;
    padding: 2px 4px;
    font-size: 0.75rem;
    min-width: 100px;
    cursor: pointer;
  }

  .fx-select:focus {
    outline: 1px solid var(--btn-active-border);
    outline-offset: 1px;
  }

  .fx-input {
    background: var(--btn-bg);
    color: var(--text-dim);
    border: 1px solid var(--btn-border);
    border-radius: 3px;
    padding: 2px 4px;
    font-size: 0.75rem;
    width: 56px;
    text-align: right;
  }

  .fx-input:focus {
    outline: 1px solid var(--btn-active-border);
    outline-offset: 1px;
  }

  .fx-apply-btn {
    align-self: flex-end;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 4px;
    padding: 3px 12px;
    font-size: 0.75rem;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.12s;
  }

  .fx-apply-btn:hover {
    opacity: 0.85;
  }
</style>
