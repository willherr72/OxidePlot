/**
 * renderer.ts — thin TypeScript wrapper around the WASM `OxidePlot` class.
 *
 * Task 3.3 — isolates all WASM imports so the rest of the app never touches
 * the raw WASM glue code.
 *
 * Task 4.1 — adds pan, zoom, autoFit, and viewState wrappers for interactive
 * canvas-driven interaction.
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

export interface ViewState {
  x_min: number;
  x_max: number;
  y_min: number;
  y_max: number;
}

export interface TableColumn {
  name: string;
  numeric: boolean;
}

export interface SeriesInfoEntry {
  name: string;
  color: [number, number, number, number];
  visible: boolean;
}

export interface TickEntry {
  value: number;
  label: string;
  major: boolean;
}

export interface AxisTicksData {
  x: TickEntry[];
  y: TickEntry[];
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

  /**
   * Auto-fit the view to encompass all series data and re-render.
   * Delegates to `auto_fit` on the WASM side (which calls render internally).
   */
  autoFit(): void {
    this.assertPlot();
    this.plot!.auto_fit();
  }

  /**
   * Pan the view by a backing-store pixel delta and re-render.
   * @param dx - horizontal drag delta in canvas backing-store pixels
   * @param dy - vertical drag delta in canvas backing-store pixels
   */
  pan(dx: number, dy: number): void {
    this.assertPlot();
    this.plot!.pan(dx, dy);
  }

  /**
   * Zoom around a screen-space anchor and re-render.
   * @param scrollY - scroll magnitude; positive = zoom in.  Pass `-event.deltaY`.
   * @param x - anchor X in canvas backing-store pixels
   * @param y - anchor Y in canvas backing-store pixels
   */
  zoom(scrollY: number, x: number, y: number): void {
    this.assertPlot();
    this.plot!.zoom(scrollY, x, y);
  }

  /** Return the current view bounds as `{ x_min, x_max, y_min, y_max }`. */
  viewState(): ViewState {
    this.assertPlot();
    return this.plot!.view_state() as ViewState;
  }

  /** Return tick data for both axes as `{ x: [...], y: [...] }`. */
  axisTicks(): AxisTicksData {
    this.assertPlot();
    return (this.plot as any).axis_ticks() as AxisTicksData;
  }

  /**
   * Set the draw mode for all existing series and re-render.
   * @param mode - one of 'lines', 'step', or 'points'
   */
  setDrawMode(mode: 'lines' | 'step' | 'points'): void {
    this.assertPlot();
    this.plot!.set_draw_mode(mode);
  }

  /**
   * Return an array of `{ name, color, visible }` for each series in render order.
   */
  seriesInfo(): SeriesInfoEntry[] {
    this.assertPlot();
    return (this.plot as any).series_info() as SeriesInfoEntry[];
  }

  /**
   * Set the visibility of a series by index and re-render.
   */
  setSeriesVisible(index: number, visible: boolean): void {
    this.assertPlot();
    (this.plot as any).set_series_visible(index, visible);
  }

  /**
   * Remove the series at `index` and re-render.
   */
  removeSeries(index: number): void {
    this.assertPlot();
    (this.plot as any).remove_series(index);
  }

  /**
   * Move the series at `from` to position `to` (reorders z-order) and re-render.
   */
  moveSeries(from: number, to: number): void {
    this.assertPlot();
    (this.plot as any).move_series(from, to);
  }

  /**
   * Set the line width for all series and re-render.
   * @param w - line width in pixels (e.g. 0.5–6)
   */
  setLineWidth(w: number): void {
    this.assertPlot();
    (this.plot as any).set_line_width(w);
  }

  /**
   * Set the point radius for all series and re-render.
   * @param r - point radius in pixels (e.g. 1–10)
   */
  setPointRadius(r: number): void {
    this.assertPlot();
    (this.plot as any).set_point_radius(r);
  }

  /**
   * Set the WebGPU clear colour to match the current theme.
   *
   * Call this when the theme changes (or on mount after loading prefs).
   * Values are linear RGBA in [0, 1].
   *  Dark:  (0.10, 0.10, 0.12, 1)
   *  Light: (0.97, 0.97, 0.98, 1)
   *
   * Note: does NOT call render() — caller should call render() after if needed.
   */
  setBackground(r: number, g: number, b: number, a: number): void {
    this.assertPlot();
    (this.plot as any).set_background(r, g, b, a);
  }

  /**
   * Export all loaded series as a CSV string.
   *
   * The header row contains the X column name followed by each series' Y name.
   * Data rows contain full (un-downsampled) values; empty cells for series
   * shorter than the longest one.
   *
   * Returns an empty string if no series have been loaded.
   */
  exportCsv(): string {
    this.assertPlot();
    return (this.plot as any).export_csv() as string;
  }

  /**
   * Enable or disable normalized multi-unit overlay mode.
   *
   * When on, each series' Y is mapped to [0, 1] using its own global min/max
   * so series with different units/scales overlay comparably on a unitless Y
   * axis.  When off, raw Y values and real axis units are restored.
   *
   * Internally calls `set_normalized` on the WASM side, which triggers
   * `auto_fit()` and `render()` automatically.
   */
  setNormalized(on: boolean): void {
    this.assertPlot();
    (this.plot as any).set_normalized(on);
  }

  /**
   * Append a derived series built from a math transform of the source at `sourceIndex`.
   *
   * `kind` — one of `'moving_average'`, `'derivative'`, `'integral'`,
   *           `'normalize'`, `'abs'`, `'log'`, `'sqrt'`.
   * `params` — optional `{ window?, mode? }`; pass `null` to use defaults.
   *
   * The new series is added, auto-fitted, and rendered immediately.
   * Throws if `sourceIndex` is out of range or `kind` is unrecognised.
   */
  addTransform(
    sourceIndex: number,
    kind: string,
    params: { window?: number; mode?: string } | null,
  ): void {
    this.assertPlot();
    (this.plot as any).add_transform(sourceIndex, kind, params);
  }

  /**
   * Set the view's X bounds (leaving Y unchanged), rebuild visible series,
   * and re-render.  Called by Graph.applyXRange when syncing X across graphs.
   */
  setXRange(xMin: number, xMax: number): void {
    this.assertPlot();
    (this.plot as any).set_x_range(xMin, xMax);
  }

  // ── Table API ─────────────────────────────────────────────────────────────

  /** Return column metadata `[{ name, numeric }]` for the loaded file. */
  tableColumns(): TableColumn[] {
    this.assertPlot();
    return this.plot!.table_columns() as TableColumn[];
  }

  /** Sort the table by column index and direction; rebuilds the view index. */
  tableSetSort(col: number, ascending: boolean): void {
    this.assertPlot();
    this.plot!.table_set_sort(col, ascending);
  }

  /** Clear sort, restoring original row order; rebuilds the view index. */
  tableClearSort(): void {
    this.assertPlot();
    this.plot!.table_clear_sort();
  }

  /** Set the global full-text search term; rebuilds the view index. */
  tableSetSearch(term: string): void {
    this.assertPlot();
    this.plot!.table_set_search(term);
  }

  /**
   * Set or clear a per-column filter; rebuilds the view index.
   * Pass `null` to clear the filter for `col`.
   * Pass `{ text }` for a substring match, `{ min?, max? }` for a numeric range.
   */
  tableSetColumnFilter(
    col: number,
    spec: { text?: string; min?: number; max?: number } | null,
  ): void {
    this.assertPlot();
    this.plot!.table_set_column_filter(col, spec);
  }

  /** Return the number of rows visible under the current filters/sort. */
  tableRowCount(): number {
    this.assertPlot();
    return this.plot!.table_row_count();
  }

  /**
   * Return a window of rows `[start, start+count)` from the current view
   * as `string[][]` — each inner array is one row, each string one cell.
   */
  tableWindow(start: number, count: number): string[][] {
    this.assertPlot();
    return this.plot!.table_window(start, count) as string[][];
  }

  private assertPlot(): void {
    if (!this.plot) throw new Error('Renderer not created — call create(canvas) first');
  }
}
