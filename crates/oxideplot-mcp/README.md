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
| `describe_data` | Per-column statistics (count, min, max, mean, median, std, peak-to-peak) |
| `query_data` | A page of raw rows, with sort / case-insensitive search / paging |
| `create_graph` | Pick X + one-or-more Y columns (by name or index) → `graph_id` |
| `render_graph` | Render the graph to a **PNG image** (with baked axis tick labels) + a text block (ranges, ticks, legend) |

The intended loop: `load_csv → describe_data` / `query_data` (understand) →
`create_graph → render_graph` (see it) → refine.

### Render options (`create_graph` / `render_graph`)

- **`layout`** — `overlay` (shared Y, default), `normalized` (each series rescaled
  0–1 to compare shapes), or `stacked` (one panel per series with its own Y axis,
  sharing X — best when scales differ, e.g. temperature vs pressure).
- **`transform`** (+ `transform_window`) — `moving_average` (smoothing),
  `derivative` (dy/dx), or `integral`, applied to every Y series before plotting.
- **`draw_mode`** — `lines` (default), `step`, or `points`.

`render_graph` accepts `layout` / `transform` overrides to re-render the same
graph differently. Large series are automatically LTTB-downsampled to ~2×width for
the render (so multi-million-row files render fast) — the text reports
`points_per_series` and `downsampled_for_render`.

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
