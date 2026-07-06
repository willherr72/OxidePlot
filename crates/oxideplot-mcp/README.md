# oxideplot-mcp

An **MCP server** that lets Claude drive OxidePlot: load a dataset, understand it
(statistics, raw rows), and **build + render plots to images Claude can see** —
reusing OxidePlot's GPU rendering engine headlessly.

It speaks the [Model Context Protocol](https://modelcontextprotocol.io) over
stdio, so it plugs into Claude Code, Claude Desktop, or any MCP client.

## Tools

| Tool | Purpose |
|------|---------|
| `load_csv` | Parse a CSV/Excel file → `dataset_id` + columns (name, kind) + row count |
| `describe_data` | Per-column stats + **QC**: n_missing, pct_zero, distinct, longest_constant_run (flag dead/frozen/duplicate channels), plus min/max/mean/median/std for numeric. All columns by default |
| `query_data` | A page of raw rows, with sort / case-insensitive search / paging |
| `correlate` | Pearson correlation matrix + pairs sorted by \|r\| (spot a decorrelated/damaged axis) |
| `derive_column` | Add a computed column: `magnitude` √(x²+…), `add`/`mean`, `subtract`/`ratio`, `scale` |
| `create_graph` | Pick X + one-or-more Y columns (by name or index) → `graph_id` |
| `render_graph` | Render to a **PNG** (baked axis tick labels, datetime-aware X) + a text block (ranges, ticks, legend) |

The intended loop: `load_csv → describe_data` / `query_data` / `correlate`
(understand) → `create_graph → render_graph` (see it) → refine.

### Render options (`create_graph` / `render_graph`)

- **`layout`** — `overlay` (shared Y, default), `normalized` (each series 0–1 to
  compare shapes), or `stacked` (one panel per series with its own Y, sharing X).
- **`transform`** (+ `transform_window`) — `moving_average`, `derivative`, `integral`.
- **`draw_mode`** — `lines` (default), `step`, or `points` (use `points` + a
  numeric X for an XY scatter).

`render_graph`-only options:

- **`row_start` / `row_end`** or **`x_min` / `x_max`** — window to a row range or
  X range (e.g. inspect a single-sample glitch); the window isn't downsampled
  unless still huge, so spikes survive.
- **`downsample`** — `minmax` (default — keeps each bucket's min & max so a
  1-sample spike is never dropped; best for QC), `lttb` (smoother), or `none`.
- **`autoscale`** — `minmax` (default) or `robust` (clip Y to the 1st–99th
  percentile so a lone outlier doesn't flatten the signal).
- **`y_scale`** — `linear` (default) or `log`.

A datetime X column is auto-detected and gets real date/time tick labels. Large
series are downsampled to ~2×width (text reports `points_per_series` /
`downsampled_for_render`), so multi-million-row files render fast.

## Build

```bash
cargo build --release -p oxideplot-mcp
# binary: target/release/oxideplot-mcp(.exe)
```

Rendering uses the GPU (wgpu), so the machine running the server needs a working
graphics adapter (any modern desktop GPU / integrated graphics is fine).

## Register with Claude Code

```bash
# Windows
claude mcp add oxideplot -- "C:/Users/WilliamHerr/Desktop/Code/OxidePlot/target/release/oxideplot-mcp.exe"

# macOS/Linux
claude mcp add oxideplot -- /path/to/OxidePlot/target/release/oxideplot-mcp
```

Then, in a Claude session: *"Load `C:/data/run.csv`, describe it, and plot the
temperature and pressure columns against time."*

## Register with Claude Desktop

Add to `claude_desktop_config.json` (Settings → Developer → Edit Config):

```json
{
  "mcpServers": {
    "oxideplot": {
      "command": "C:/Users/WilliamHerr/Desktop/Code/OxidePlot/target/release/oxideplot-mcp.exe"
    }
  }
}
```

Restart Claude Desktop; the OxidePlot tools appear under the 🔌 menu.

## Notes

- **Absolute paths:** `load_csv` reads from the server's working directory, so
  pass absolute file paths.
- **Session state:** datasets and graphs live in memory for the life of the
  server process (ids like `ds-1`, `gr-2`).
- **Axis labels:** numeric tick labels are drawn onto the PNG (per panel). The
  title and series legend come back in the text companion (they'd need a full
  letterform font to bake into the image).

## Verify without a client

```bash
# initialize + list tools over stdio
printf '%s\n' \
 '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"c","version":"0"}}}' \
 '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
 '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
 | ./target/release/oxideplot-mcp
```
