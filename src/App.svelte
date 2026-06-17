<script lang="ts">
  import { onMount } from 'svelte';
  import { pickFile, readFile } from './lib/api.js';
  import { Renderer } from './lib/renderer.js';
  import type { FileMeta, SeriesSpec } from './lib/renderer.js';
  import ColumnDialog from './lib/components/ColumnDialog.svelte';

  let canvas: HTMLCanvasElement;
  const renderer = new Renderer();

  let fileMeta: FileMeta | null = null;
  let filePath: string | null = null;
  let error: string | null = null;
  let loading = false;

  onMount(async () => {
    try {
      await renderer.init();
      await renderer.create(canvas);
      renderer.render(); // blank dark frame
    } catch (e) {
      error = String(e);
    }

    // Keep the GPU surface in sync with the canvas element size.
    const ro = new ResizeObserver(entries => {
      for (const entry of entries) {
        const { width, height } = entry.contentRect;
        if (width > 0 && height > 0) {
          canvas.width = Math.round(width);
          canvas.height = Math.round(height);
          renderer.resize(canvas.width, canvas.height);
          renderer.render();
        }
      }
    });
    ro.observe(canvas);
    return () => ro.disconnect();
  });

  async function handleOpen() {
    error = null;
    loading = true;
    try {
      const path = await pickFile();
      if (!path) { loading = false; return; }
      filePath = path;

      const numArr = await readFile(path);
      const bytes = new Uint8Array(numArr);
      const filename = path.split(/[\\/]/).pop() ?? path;

      fileMeta = renderer.loadFileBytes(bytes, filename);
    } catch (e) {
      error = `Failed to open file: ${e}`;
    } finally {
      loading = false;
    }
  }

  function handleConfirm(event: CustomEvent<SeriesSpec[]>) {
    const specs = event.detail;
    fileMeta = null; // close dialog
    error = null;
    try {
      renderer.setSeries(specs);
    } catch (e) {
      error = `Failed to render series: ${e}`;
    }
  }

  function handleCancel() {
    fileMeta = null;
  }
</script>

<main>
  <!-- Toolbar -->
  <div class="toolbar">
    <button class="open-btn" on:click={handleOpen} disabled={loading}>
      {loading ? 'Loading…' : 'Open File'}
    </button>
    {#if filePath && !fileMeta}
      <span class="file-label" title={filePath}>
        {filePath.split(/[\\/]/).pop()}
      </span>
    {/if}
    {#if error}
      <span class="error-msg">{error}</span>
    {/if}
  </div>

  <!-- Plot canvas — fills the remaining space -->
  <canvas bind:this={canvas}></canvas>

  <!-- Column-selection dialog -->
  {#if fileMeta}
    <ColumnDialog
      meta={fileMeta}
      on:confirm={handleConfirm}
      on:cancel={handleCancel}
    />
  {/if}
</main>

<style>
  :global(body) {
    margin: 0;
    background: #1a1a1f;
    overflow: hidden;
    color: #e0e0ee;
    font-family: sans-serif;
  }

  main {
    width: 100vw;
    height: 100vh;
    display: flex;
    flex-direction: column;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    background: #111118;
    border-bottom: 1px solid #2a2a3a;
    flex-shrink: 0;
    height: 42px;
    box-sizing: border-box;
  }

  .open-btn {
    padding: 5px 16px;
    background: #3060c0;
    color: #fff;
    border: none;
    border-radius: 5px;
    cursor: pointer;
    font-size: 0.85rem;
    font-weight: 600;
    transition: opacity 0.15s;
  }

  .open-btn:hover:not(:disabled) {
    opacity: 0.85;
  }

  .open-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .file-label {
    font-size: 0.8rem;
    color: #8888aa;
    max-width: 400px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .error-msg {
    font-size: 0.8rem;
    color: #ff6666;
    max-width: 600px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  canvas {
    display: block;
    flex: 1;
    width: 100%;
    /* height is controlled by flex; the ResizeObserver keeps canvas px in sync */
  }
</style>
