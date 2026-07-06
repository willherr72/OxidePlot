# MCP M2 — `oxideplot-mcp` Server Design

**Date:** 2026-06-18
**Status:** Approved direction → plan next
**Depends on:** M1 (`oxideplot-core::render::PlotRenderer::render_to_rgba`).

## Goal

A Rust MCP server (`oxideplot-mcp`) that lets Claude drive OxidePlot over stdio:
load a dataset, understand it three ways (stats / raw rows / a rendered image),
and iterate. Reuses `oxideplot-core` directly (parsing, `data::table`,
`processing::statistics`, `render` + M1 offscreen). In-memory stateful sessions
(no IPC, no temp files).

## Crate

New workspace member `crates/oxideplot-mcp` (binary). Add to the workspace
`members`. Dependencies:
- `rmcp = { version = "0.16", features = ["server", "transport-io"] }` (stdio) —
  resolve exact feature names against the compiler; `transport::stdio` may live
  behind `transport-io`/`server`.
- `tokio` (rt-multi-thread, macros), `serde`, `serde_json`, `schemars`, `anyhow`,
  `image` (PNG encode — this crate MAY depend on `image`; it's native-only),
  `oxideplot-core` (path).

## Server state

Handler struct `OxidePlot` (`#[derive(Clone)]`) wrapping
`Arc<Mutex<Session>>`:
```
Session {
  datasets: HashMap<String, Dataset>,   // id -> parsed data
  graphs:   HashMap<String, GraphSpec>, // id -> graph config
  next: u64,                            // id counter (ds-1, gr-1, …)
}
Dataset { name: String, data: LoadedData, meta: FileMeta } // from core loader
GraphSpec { dataset_id, x_col: usize, y_cols: Vec<usize>, opts: GraphOpts }
GraphOpts { draw_mode, normalize, title, width, height }   // sane defaults
```
Ids are opaque strings the tools echo back. Tools lock the mutex briefly.

## Tools (rmcp `#[tool]` methods; params are `JsonSchema` structs)

1. **`load_csv(path: String)`** → parse the file via `load_from_bytes` (read
   bytes from disk; the server runs locally). Store a `Dataset`. Return JSON text:
   `{ dataset_id, name, rows, columns: [{index, name, kind}] }`.
2. **`describe_data(dataset_id, columns?: [String])`** → per requested (or all
   numeric) column, compute count / min / max / mean / std / n_missing via
   `processing::statistics` + `column_to_f64`. Return a JSON table of stats.
3. **`query_data(dataset_id, sort_col?, sort_desc?, search?, offset?, limit?)`**
   → reuse Phase 7 `data::table` (`TableQuery` + `compute_view_index` +
   `window_rows`) to return `{ total, columns, rows: [[cell,…],…] }` (a page).
   `limit` defaults to 20, capped (e.g. 200) to keep responses small.
4. **`create_graph(dataset_id, x_col, y_cols, draw_mode?, normalize?, title?, width?, height?)`**
   — `x_col`/`y_cols` accept column NAME or index (resolve to index; validate
   they exist + are plottable). Store a `GraphSpec`. Return `{ graph_id, x, ys,
   size }`.
5. **`render_graph(graph_id, width?, height?)`** — the payoff:
   - Build series: `xs = column_to_f64/column_to_timestamps(x_col)`,
     `ys = column_to_f64(y_col)` for each y; assemble `SeriesGpuData` (colors from
     the shared palette) + a `GridGpuData` from `render::axis::compute_grid_lines`
     over the fitted view.
   - Fit the view to the (visible) data; `new_offscreen(w,h)` (defaults 900×560);
     `build_draw_calls`; `render_to_rgba(clear = graphite)`.
   - Encode PNG (`image::RgbaImage` → PNG bytes).
   - Return **image content** (PNG, base64 via rmcp's `Content::image`) PLUS a
     text block: title, x-range + x tick values, y-range + y tick values, and the
     legend (series name → color) — so Claude can read the SCALES even though the
     GPU layer draws no axis labels yet.

## Axis labels (scope)

The GPU render draws lines + grid but **not** tick labels (those are SVG overlays
in the app). For M2, the returned **text** carries the ranges + tick values +
legend, so the image + text together are fully interpretable. Compositing tick
labels directly onto the PNG (a text-rendering crate over `compute_grid_lines`/
`format_tick_value`) is an **M3** enhancement.

## Rendering notes

- No downsampling for M2 — a static render of even ~1e5 points is fine on the
  GPU and stays accurate. (`downsample_for_view` is available if needed later.)
- Reuse the app's clear color (`[0.055, 0.059, 0.075, 1.0]`) and 8-color palette.
- `render_graph` builds a fresh `new_offscreen` per call (matches the M1 model).

## Transport & setup

`#[tokio::main]`; `OxidePlot::default().serve(stdio()).await?; .waiting().await`.
Ship a README section: `claude mcp add oxideplot -- <path>/oxideplot-mcp` (and the
Claude Desktop JSON equivalent).

## Testing

- Rust integration tests on the **logic** (not the MCP transport): a helper builds
  a `Session`, calls the tool bodies (factored so the core logic is callable
  without the rmcp layer), and asserts: load returns the right column count;
  describe stats match a hand-computed fixture; query returns the right page/order
  (reuses table tests); render returns non-empty PNG bytes of the requested size.
- A smoke test that the server binary starts and responds to `initialize` +
  `tools/list` over stdio (optional; can be manual via `claude mcp`).
- Manual: `claude mcp add` it, then drive the loop from a Claude session.

## Decomposition (plan tasks)

1. **Crate skeleton + rmcp over stdio** — one trivial `ping` tool; compiles,
   `tools/list` works. *(Nails the SDK API before real tools.)*
2. **Data tools** — `load_csv`, `describe_data`, `query_data` (+ Session state) + logic tests.
3. **Graph tools** — `create_graph`, `render_graph` (offscreen → PNG → image content + text) + tests.
4. **Docs/packaging** — README `claude mcp add` instructions; workspace wiring.

## Non-goals (M2)

Baked-in axis-label PNG compositing (M3), transforms/resample tools (M3),
multi-graph compositing, inline-CSV/URL loading, auth, remote transport.
