<script lang="ts">
  import { onMount } from 'svelte';
  import init, { OxidePlot } from './lib/wasm/oxideplot_wasm.js';
  import wasmUrl from './lib/wasm/oxideplot_wasm_bg.wasm?url';

  let canvas: HTMLCanvasElement;

  onMount(async () => {
    await init({ module_or_path: wasmUrl });
    const plot = await OxidePlot.create(canvas);
    plot.render();
  });
</script>

<main>
  <canvas bind:this={canvas} width="1200" height="700"></canvas>
</main>

<style>
  :global(body) {
    margin: 0;
    background: #1a1a1f;
    overflow: hidden;
  }

  main {
    width: 100vw;
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  canvas {
    display: block;
    width: 1200px;
    height: 700px;
    max-width: 100vw;
    max-height: 100vh;
  }
</style>
