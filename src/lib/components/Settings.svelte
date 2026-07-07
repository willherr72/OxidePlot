<script lang="ts">
  /**
   * Settings.svelte — floating settings panel for plot appearance.
   *
   * Controls:
   *   - Line width (range slider + number, 0.5–6)
   *   - Point radius (range slider + number, 1–10)
   *   - Grid on/off (checkbox)
   *   - Normalize multi-unit (checkbox)
   *   - Autoscale mode (select: minmax / robust)
   *   - Y-scale (select: linear / log)
   *   - Downsample mode (select: minmax / lttb / none)
   *
   * Emits:
   *   - linewidth: { value: number }
   *   - pointradius: { value: number }
   *   - showgrid: { value: boolean }
   *   - normalized: { value: boolean }
   *   - autoscalemode: { value: string }
   *   - yscale: { value: string }
   *   - downsamplemode: { value: string }
   *
   * Colors use CSS custom properties so the panel responds to data-theme.
   */
  import { createEventDispatcher } from 'svelte';

  export let lineWidth: number = 2.0;
  export let pointRadius: number = 3.0;
  export let showGrid: boolean = true;
  export let normalized: boolean = false;
  export let autoscaleMode: string = 'minmax';
  export let yScale: string = 'linear';
  export let downsampleMode: string = 'minmax';

  const dispatch = createEventDispatcher<{
    linewidth: { value: number };
    pointradius: { value: number };
    showgrid: { value: boolean };
    normalized: { value: boolean };
    autoscalemode: { value: string };
    yscale: { value: string };
    downsamplemode: { value: string };
  }>();

  function onLineWidthChange() {
    dispatch('linewidth', { value: lineWidth });
  }

  function onPointRadiusChange() {
    dispatch('pointradius', { value: pointRadius });
  }

  function onShowGridChange() {
    dispatch('showgrid', { value: showGrid });
  }

  function onNormalizedChange() {
    dispatch('normalized', { value: normalized });
  }

  function onAutoscaleModeChange(e: Event) {
    dispatch('autoscalemode', { value: (e.currentTarget as HTMLSelectElement).value });
  }

  function onYScaleChange(e: Event) {
    dispatch('yscale', { value: (e.currentTarget as HTMLSelectElement).value });
  }

  function onDownsampleModeChange(e: Event) {
    dispatch('downsamplemode', { value: (e.currentTarget as HTMLSelectElement).value });
  }
</script>

<div class="settings-panel">
  <div class="settings-header">Settings</div>

  <div class="setting-row">
    <label for="line-width">Line width</label>
    <div class="input-group">
      <input
        id="line-width"
        type="range"
        min="0.5"
        max="6"
        step="0.5"
        bind:value={lineWidth}
        on:input={onLineWidthChange}
      />
      <span class="val-label">{lineWidth.toFixed(1)}</span>
    </div>
  </div>

  <div class="setting-row">
    <label for="point-radius">Point radius</label>
    <div class="input-group">
      <input
        id="point-radius"
        type="range"
        min="1"
        max="10"
        step="0.5"
        bind:value={pointRadius}
        on:input={onPointRadiusChange}
      />
      <span class="val-label">{pointRadius.toFixed(1)}</span>
    </div>
  </div>

  <div class="setting-row checkbox-row">
    <label for="show-grid">Show grid</label>
    <input
      id="show-grid"
      type="checkbox"
      bind:checked={showGrid}
      on:change={onShowGridChange}
    />
  </div>

  <div class="setting-row checkbox-row">
    <label for="normalize">Normalize (multi-unit)</label>
    <input
      id="normalize"
      type="checkbox"
      bind:checked={normalized}
      on:change={onNormalizedChange}
    />
  </div>

  <div class="setting-row">
    <label for="autoscale-mode">Autoscale</label>
    <select id="autoscale-mode" value={autoscaleMode} on:change={onAutoscaleModeChange}>
      <option value="minmax">Min / Max</option>
      <option value="robust">Robust</option>
    </select>
  </div>

  <div class="setting-row">
    <label for="y-scale">Y-scale</label>
    <select id="y-scale" value={yScale} on:change={onYScaleChange}>
      <option value="linear">Linear</option>
      <option value="log">Log</option>
    </select>
  </div>

  <div class="setting-row">
    <label for="downsample-mode">Downsample</label>
    <select id="downsample-mode" value={downsampleMode} on:change={onDownsampleModeChange}>
      <option value="minmax">Min / Max</option>
      <option value="lttb">LTTB</option>
      <option value="none">None</option>
    </select>
  </div>
</div>

<style>
  .settings-panel {
    position: absolute;
    top: 8px;
    left: 8px;
    z-index: 100;
    background: var(--panel-bg);
    border: 1px solid var(--border-mid);
    border-radius: var(--radius);
    padding: 12px 16px;
    min-width: 220px;
    box-shadow: var(--shadow-panel);
    color: var(--text-dim);
    font-family: var(--font-ui);
    font-size: 0.8rem;
  }

  .settings-header {
    font-size: 0.88rem;
    font-weight: 700;
    color: var(--settings-header);
    margin-bottom: 10px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .setting-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
    gap: 10px;
  }

  .setting-row label {
    flex-shrink: 0;
    min-width: 90px;
    color: var(--settings-label);
  }

  .input-group {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
  }

  input[type="range"] {
    flex: 1;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .val-label {
    font-size: 0.78rem;
    color: var(--settings-val);
    min-width: 28px;
    text-align: right;
  }

  .checkbox-row {
    margin-top: 4px;
  }

  input[type="checkbox"] {
    width: 15px;
    height: 15px;
    accent-color: var(--accent);
    cursor: pointer;
    margin-right: auto;
  }

  select {
    flex: 1;
    background: var(--btn-bg);
    color: var(--text-dim);
    border: 1px solid var(--btn-border);
    border-radius: var(--radius-sm);
    padding: 3px 6px;
    font-family: var(--font-ui);
    font-size: 0.78rem;
    cursor: pointer;
  }

  select:hover {
    border-color: var(--border-mid);
  }
</style>
