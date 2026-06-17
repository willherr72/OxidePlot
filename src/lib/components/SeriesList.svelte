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
</script>

<div class="series-panel">
  <div class="panel-header">Series</div>
  <ul class="series-list">
    {#each series as s, i}
      <li class="series-row" class:hidden={!s.visible}>
        <!-- Color swatch -->
        <span class="swatch" style="background:{toCSS(s.color)}"></span>
        <!-- Name -->
        <span class="series-name" title={s.name}>{s.name}</span>
        <!-- Controls -->
        <span class="controls">
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
      </li>
    {/each}
  </ul>
</div>

<style>
  .series-panel {
    position: absolute;
    top: 8px;
    right: 8px;
    background: rgba(20, 20, 30, 0.88);
    border: 1px solid #3a3a50;
    border-radius: 8px;
    min-width: 180px;
    max-width: 260px;
    max-height: 320px;
    display: flex;
    flex-direction: column;
    font-family: sans-serif;
    font-size: 0.8rem;
    color: #c0c0de;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.5);
    z-index: 20;
    overflow: hidden;
    backdrop-filter: blur(4px);
  }

  .panel-header {
    padding: 6px 10px;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: #8888aa;
    border-bottom: 1px solid #2a2a3a;
    flex-shrink: 0;
  }

  .series-list {
    list-style: none;
    margin: 0;
    padding: 4px 0;
    overflow-y: auto;
  }

  .series-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    transition: background 0.12s;
  }

  .series-row:hover {
    background: rgba(60, 60, 90, 0.4);
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
    border: 1px solid rgba(255, 255, 255, 0.15);
  }

  .series-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.78rem;
    color: #d0d0ee;
  }

  .controls {
    display: flex;
    gap: 2px;
    flex-shrink: 0;
  }

  .ctrl-btn {
    background: transparent;
    border: none;
    color: #8888aa;
    cursor: pointer;
    padding: 1px 4px;
    font-size: 0.8rem;
    border-radius: 3px;
    line-height: 1.2;
    transition: color 0.12s, background 0.12s;
  }

  .ctrl-btn:hover:not(:disabled) {
    background: rgba(80, 80, 120, 0.5);
    color: #e0e0ff;
  }

  .ctrl-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .remove-btn:hover:not(:disabled) {
    background: rgba(160, 40, 40, 0.5);
    color: #ff8888;
  }
</style>
