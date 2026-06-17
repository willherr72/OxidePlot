<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { FileMeta, SeriesSpec } from '../renderer.js';

  export let meta: FileMeta;

  const dispatch = createEventDispatcher<{ confirm: SeriesSpec[]; cancel: void }>();

  // Colour palette (RGBA f32) for up to 8 Y columns.
  const PALETTE: [number, number, number, number][] = [
    [0.20, 0.85, 1.00, 1.0], // bright cyan
    [1.00, 0.60, 0.10, 1.0], // amber
    [0.40, 1.00, 0.40, 1.0], // lime green
    [1.00, 0.30, 0.30, 1.0], // coral
    [0.80, 0.40, 1.00, 1.0], // violet
    [1.00, 0.90, 0.10, 1.0], // yellow
    [0.10, 0.90, 0.70, 1.0], // teal
    [1.00, 0.55, 0.80, 1.0], // pink
  ];

  // Default X to first numeric/datetime column, Y to everything else.
  let xCol: number = meta.columns.findIndex(c => c.kind !== 'text');
  if (xCol < 0) xCol = 0;

  let ySelected: boolean[] = meta.columns.map((_, i) => i !== xCol && meta.columns[i].kind !== 'text');

  function onConfirm() {
    const specs: SeriesSpec[] = [];
    let colorIdx = 0;
    for (let i = 0; i < meta.columns.length; i++) {
      if (ySelected[i] && i !== xCol) {
        specs.push({
          x_col: xCol,
          y_col: i,
          color: PALETTE[colorIdx % PALETTE.length],
          draw_mode: 'lines',
        });
        colorIdx++;
      }
    }
    if (specs.length === 0) {
      alert('Please select at least one Y column.');
      return;
    }
    dispatch('confirm', specs);
  }

  function onCancel() {
    dispatch('cancel');
  }

  // When X selection changes, deselect that column from Y.
  $: ySelected = ySelected.map((v, i) => (i === xCol ? false : v));
</script>

<div class="overlay">
  <div class="dialog">
    <h2>Choose Columns</h2>
    <p class="subtitle">{meta.rows} rows · {meta.columns.length} columns</p>

    <div class="section">
      <label class="section-title">X Axis (time or index)</label>
      <div class="col-list">
        {#each meta.columns as col, i}
          <label class="col-row" class:selected={xCol === i} class:disabled={col.kind === 'text'}>
            <input
              type="radio"
              name="x_col"
              value={i}
              bind:group={xCol}
              disabled={col.kind === 'text'}
            />
            <span class="col-name">{col.name}</span>
            <span class="col-kind kind-{col.kind}">{col.kind}</span>
          </label>
        {/each}
      </div>
    </div>

    <div class="section">
      <label class="section-title">Y Axis (one or more)</label>
      <div class="col-list">
        {#each meta.columns as col, i}
          <label class="col-row" class:disabled={i === xCol || col.kind === 'text'}>
            <input
              type="checkbox"
              bind:checked={ySelected[i]}
              disabled={i === xCol || col.kind === 'text'}
            />
            <span class="col-name">{col.name}</span>
            <span class="col-kind kind-{col.kind}">{col.kind}</span>
          </label>
        {/each}
      </div>
    </div>

    <div class="actions">
      <button class="btn-cancel" on:click={onCancel}>Cancel</button>
      <button class="btn-confirm" on:click={onConfirm}>Plot</button>
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: var(--dialog-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }

  .dialog {
    background: var(--dialog-bg);
    border: 1px solid var(--border-mid);
    border-radius: 10px;
    padding: 24px 28px;
    min-width: 360px;
    max-width: 520px;
    color: var(--dialog-text);
    font-family: sans-serif;
    box-shadow: 0 8px 40px rgba(0, 0, 0, 0.4);
  }

  h2 {
    margin: 0 0 4px;
    font-size: 1.2rem;
    color: var(--dialog-h2);
  }

  .subtitle {
    margin: 0 0 18px;
    font-size: 0.8rem;
    color: var(--dialog-subtitle);
  }

  .section {
    margin-bottom: 18px;
  }

  .section-title {
    display: block;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--dialog-section-title);
    margin-bottom: 8px;
  }

  .col-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 180px;
    overflow-y: auto;
  }

  .col-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.15s;
  }

  .col-row:hover:not(.disabled) {
    background: var(--col-row-hover);
  }

  .col-row.selected {
    background: var(--col-row-selected);
  }

  .col-row.disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .col-name {
    flex: 1;
    font-size: 0.9rem;
  }

  .col-kind {
    font-size: 0.7rem;
    padding: 2px 6px;
    border-radius: 4px;
  }

  .kind-numeric {
    background: var(--col-kind-numeric-bg);
    color: var(--col-kind-numeric-text);
  }

  .kind-datetime {
    background: var(--col-kind-datetime-bg);
    color: var(--col-kind-datetime-text);
  }

  .kind-text {
    background: var(--col-kind-text-bg);
    color: var(--col-kind-text-text);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 22px;
  }

  button {
    padding: 8px 20px;
    border-radius: 6px;
    border: none;
    cursor: pointer;
    font-size: 0.9rem;
    font-weight: 600;
    transition: opacity 0.15s;
  }

  button:hover {
    opacity: 0.85;
  }

  .btn-cancel {
    background: var(--btn-cancel-bg);
    color: var(--btn-cancel-text);
  }

  .btn-confirm {
    background: var(--accent);
    color: #ffffff;
  }
</style>
