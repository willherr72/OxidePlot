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
          <!-- Color swatch -->
          <span class="swatch" style="background:{toCSS(s.color)}"></span>
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
              on:click={() => toggleVisible(i, !s.visible)}
            >{s.visible ? '●' : '○'}</button>
            <button
              class="ctrl-btn"
              title="Move up (lower z-order)"
              disabled={i === 0}
              on:click={() => moveUp(i)}
            >↑</button>
            <button
              class="ctrl-btn"
              title="Move down (higher z-order)"
              disabled={i === series.length - 1}
              on:click={() => moveDown(i)}
            >↓</button>
            <button
              class="ctrl-btn remove-btn"
              title="Remove series"
              on:click={() => remove(i)}
            >×</button>
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
    border-radius: 8px;
    min-width: 180px;
    max-width: 260px;
    max-height: 420px;
    display: flex;
    flex-direction: column;
    font-family: sans-serif;
    font-size: 0.8rem;
    color: var(--text-dim);
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
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

  .swatch {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 2px;
    flex-shrink: 0;
    border: 1px solid var(--swatch-border);
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
    background: transparent;
    border: none;
    color: var(--series-ctrl-btn);
    cursor: pointer;
    padding: 1px 4px;
    font-size: 0.8rem;
    border-radius: 3px;
    line-height: 1.2;
    transition: color 0.12s, background 0.12s;
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
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.02em;
    padding: 1px 5px;
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
