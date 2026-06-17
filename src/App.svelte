<script lang="ts">
  import { onMount } from 'svelte';
  import { runTriangle } from './lib/spike/webgpu_triangle';
  import init, { run_triangle } from './lib/wasm/oxideplot_wasm.js';
  import wasmUrl from './lib/wasm/oxideplot_wasm_bg.wasm?url';

  let tsCanvas: HTMLCanvasElement;
  let wasmCanvas: HTMLCanvasElement;

  let tsStatus = 'running…';
  let wasmStatus = 'running…';

  onMount(async () => {
    // --- Task 0.2: TypeScript WebGPU triangle ---
    try {
      tsStatus = await runTriangle(tsCanvas);
    } catch (e: unknown) {
      tsStatus = `TS_ERROR: ${e instanceof Error ? e.message : String(e)}`;
    }

    // --- Task 0.3: Rust/wgpu-on-wasm triangle ---
    try {
      await init({ module_or_path: wasmUrl });
      wasmStatus = await run_triangle(wasmCanvas);
    } catch (e: unknown) {
      wasmStatus = `WASM_ERROR: ${e instanceof Error ? e.message : String(e)}`;
    }
  });
</script>

<main>
  <h1>OxidePlot — Phase 0 WebGPU Spike</h1>

  <section class="probe">
    <h2>Probe 1 — TypeScript WebGPU</h2>
    <canvas bind:this={tsCanvas} width="640" height="480"></canvas>
    <p class="status" data-status={tsStatus}>TS WebGPU: {tsStatus}</p>
  </section>

  <section class="probe">
    <h2>Probe 2 — Rust/wgpu → WASM</h2>
    <canvas bind:this={wasmCanvas} width="640" height="480"></canvas>
    <p class="status" data-status={wasmStatus}>WASM WebGPU: {wasmStatus}</p>
  </section>
</main>

<style>
  main {
    font-family: system-ui, sans-serif;
    max-width: 1400px;
    margin: 0 auto;
    padding: 2rem;
    background: #0d0d0d;
    min-height: 100vh;
    color: #e0e0e0;
  }

  h1 {
    font-size: 2rem;
    margin-bottom: 2rem;
    color: #fff;
  }

  .probe {
    margin-bottom: 3rem;
  }

  h2 {
    font-size: 1.4rem;
    margin-bottom: 1rem;
    color: #aaa;
  }

  canvas {
    display: block;
    border: 2px solid #444;
    background: #000;
    max-width: 100%;
  }

  .status {
    margin-top: 0.75rem;
    font-size: 2.5rem;
    font-weight: 700;
    letter-spacing: 0.05em;
    font-family: monospace;
    color: #ff0;
  }

  /* Green when OK, red when not */
  .status[data-status="WEBGPU_OK"],
  .status[data-status="WASM_WEBGPU_OK"] {
    color: #0f0;
  }
  .status[data-status^="NO_"],
  .status[data-status^="WASM_NO_"],
  .status[data-status^="TS_ERROR"],
  .status[data-status^="WASM_ERROR"],
  .status[data-status^="WASM_SURFACE_ERR"],
  .status[data-status^="WASM_DEVICE_ERR"],
  .status[data-status^="WASM_FRAME_ERR"] {
    color: #f44;
  }
</style>
