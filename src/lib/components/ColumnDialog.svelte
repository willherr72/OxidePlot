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

  // ── Column search / bulk-select (helps with wide datasets) ──────────────────
  let search = '';

  /** True when a column matches the current search filter (case-insensitive). */
  function matches(col: { name: string }): boolean {
    const q = search.trim().toLowerCase();
    return q === '' || col.name.toLowerCase().includes(q);
  }

  /** Select every currently-visible, eligible column for Y. */
  function selectAllVisible() {
    ySelected = ySelected.map((v, i) =>
      matches(meta.columns[i]) && i !== xCol && meta.columns[i].kind !== 'text' ? true : v
    );
  }

  /** Clear Y selection for every currently-visible column. */
  function clearAllVisible() {
    ySelected = ySelected.map((v, i) => (matches(meta.columns[i]) ? false : v));
  }

  $: yCount = ySelected.filter((v, i) => v && i !== xCol).length;
</script>

<div class="overlay">
  <div class="dialog">
    <h2>Choose Columns</h2>
    <p class="subtitle">{meta.rows} rows · {meta.columns.length} columns</p>

    <input
      class="col-search"
      type="text"
      placeholder="Filter columns…"
      bind:value={search}
      aria-label="Filter columns"
    />

    <div class="section">
      <label class="section-title">X Axis (time or index)</label>
      <div class="col-list">
        {#each meta.columns as col, i}
          {#if matches(col)}
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
          {/if}
        {/each}
      </div>
    </div>

    <div class="section">
      <div class="section-head">
        <label class="section-title">Y Axis · {yCount} selected</label>
        <span class="yctl">
          <button type="button" class="mini-btn" on:click={selectAllVisible}>All</button>
          <button type="button" class="mini-btn" on:click={clearAllVisible}>None</button>
        </span>
      </div>
      <div class="col-list">
        {#each meta.columns as col, i}
          {#if matches(col)}
            <label class="col-row" class:disabled={i === xCol || col.kind === 'text'}>
              <input
                type="checkbox"
                bind:checked={ySelected[i]}
                disabled={i === xCol || col.kind === 'text'}
              />
              <span class="col-name">{col.name}</span>
              <span class="col-kind kind-{col.kind}">{col.kind}</span>
            </label>
          {/if}
        {/each}
      </div>
    </div>

    <div class="actions">
      <button class="btn-cancel" on:click={onCancel}>Cancel</button>
      <button class="btn-confirm" on:click={onConfirm}>Plot{yCount > 0 ? ` (${yCount})` : ''}</button>
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
    width: min(92vw, 640px);
    max-height: 90vh;
    overflow-y: auto;
    color: var(--dialog-text);
    font-family: var(--font-ui);
    box-shadow: var(--shadow-panel);
  }

  .col-search {
    width: 100%;
    box-sizing: border-box;
    margin-bottom: 16px;
    padding: 9px 12px;
    background: var(--bg);
    border: 1px solid var(--border-mid);
    border-radius: var(--radius-sm);
    color: var(--dialog-text);
    font-family: var(--font-ui);
    font-size: 0.85rem;
    outline: none;
  }
  .col-search:focus {
    border-color: var(--accent);
  }
  .col-search::placeholder {
    color: var(--text-muted);
  }

  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }
  .section-head .section-title {
    margin-bottom: 0;
  }
  .yctl {
    display: flex;
    gap: 6px;
  }
  .mini-btn {
    padding: 3px 10px;
    font-family: var(--font-ui);
    font-size: 0.66rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    background: var(--btn-cancel-bg);
    color: var(--btn-cancel-text);
    border: 1px solid var(--border-mid);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .mini-btn:hover {
    color: var(--accent);
    border-color: var(--accent-dim);
    opacity: 1;
  }

  h2 {
    margin: 0 0 4px;
    font-family: var(--font-display);
    font-size: 1.15rem;
    font-weight: 700;
    letter-spacing: 0.03em;
    text-transform: uppercase;
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
    gap: 3px;
    max-height: 244px;
    overflow-y: auto;
    padding-right: 4px;
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
