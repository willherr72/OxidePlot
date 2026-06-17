/**
 * renderer.ts — thin TypeScript wrapper around the WASM `OxidePlot` class.
 *
 * Task 3.3 — isolates all WASM imports so the rest of the app never touches
 * the raw WASM glue code.
 */

import initWasm, { OxidePlot } from './wasm/oxideplot_wasm.js';
import wasmUrl from './wasm/oxideplot_wasm_bg.wasm?url';

export interface ColumnMeta {
  name: string;
  kind: 'numeric' | 'datetime' | 'text';
}

export interface FileMeta {
  columns: ColumnMeta[];
  rows: number;
}

export interface SeriesSpec {
  x_col: number;
  y_col: number;
  color: [number, number, number, number];
  draw_mode: 'lines' | 'step' | 'points';
}

/**
 * Wrapper around the WASM `OxidePlot` GPU renderer.
 *
 * Typical usage:
 * ```ts
 * const r = new Renderer();
 * await r.init();
 * await r.create(canvas);
 * const meta = r.loadFileBytes(bytes, 'demo.csv');
 * r.setSeries([{ x_col: 0, y_col: 1, color: [0.2, 0.85, 1.0, 1.0], draw_mode: 'lines' }]);
 * ```
 */
export class Renderer {
  private plot: OxidePlot | null = null;
  private ready = false;

  /** Initialise the WASM module.  Must be called before `create`. */
  async init(): Promise<void> {
    await initWasm({ module_or_path: wasmUrl });
    this.ready = true;
  }

  /** Create the GPU plot surface bound to `canvas`. */
  async create(canvas: HTMLCanvasElement): Promise<void> {
    if (!this.ready) throw new Error('Call init() before create()');
    this.plot = await OxidePlot.create(canvas);
  }

  /**
   * Parse file bytes and return column metadata.
   * Throws a string error if parsing fails or WASM returns an error.
   */
  loadFileBytes(bytes: Uint8Array, filename: string): FileMeta {
    this.assertPlot();
    const result = this.plot!.load_file_bytes(bytes as unknown as Uint8Array, filename);
    if (result === undefined || result === null) {
      throw new Error('load_file_bytes returned nothing');
    }
    return result as FileMeta;
  }

  /**
   * Build GPU series from specs, auto-fit the view, and re-render.
   * Throws if no file has been loaded or the spec JSON is invalid.
   */
  setSeries(specs: SeriesSpec[]): void {
    this.assertPlot();
    const json = JSON.stringify(specs);
    this.plot!.set_series(json);
  }

  /** Render one frame (use after pan/zoom events, if not called by setSeries). */
  render(): void {
    this.assertPlot();
    this.plot!.render();
  }

  /** Notify the renderer of a canvas resize. */
  resize(w: number, h: number): void {
    this.assertPlot();
    this.plot!.resize(w, h);
  }

  /** Auto-fit the view to encompass all series data. */
  autoFit(): void {
    this.assertPlot();
    this.plot!.auto_fit();
  }

  private assertPlot(): void {
    if (!this.plot) throw new Error('Renderer not created — call create(canvas) first');
  }
}
