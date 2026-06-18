<script lang="ts">
  /**
   * Settings.svelte — floating settings panel for plot appearance.
   *
   * Controls:
   *   - Line width (range slider + number, 0.5–6)
   *   - Point radius (range slider + number, 1–10)
   *   - Grid on/off (checkbox)
   *   - Normalize multi-unit (checkbox)
   *
   * Emits:
   *   - linewidth: { value: number }
   *   - pointradius: { value: number }
   *   - showgrid: { value: boolean }
   *   - normalized: { value: boolean }
   *
   * Colors use CSS custom properties so the panel responds to data-theme.
   */
  import { createEventDispatcher } from 'svelte';

  export let lineWidth: number = 2.0;
  export let pointRadius: number = 3.0;
  export let showGrid: boolean = true;
  export let normalized: boolean = false;

  const dispatch = createEventDispatcher<{
    linewidth: { value: number };
    pointradius: { value: number };
    showgrid: { value: boolean };
    normalized: { value: boolean };
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
</div>

<style>
  .settings-panel {
    position: absolute;
    top: 8px;
    right: 8px;
    z-index: 100;
    background: var(--panel-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 12px 16px;
    min-width: 220px;
    box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
    color: var(--text-dim);
    font-family: sans-serif;
    font-size: 0.82rem;
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
</style>
