# OxidePlot

A high-performance data visualization tool built with Rust. OxidePlot uses GPU-accelerated rendering via `wgpu` and `egui` to plot large datasets with smooth interactivity.

## Features

- **GPU-accelerated 2D and 3D plotting** — Custom `wgpu` shaders for lines, steps, and scatter points
- **CSV and Excel import** — Drag-and-drop or file dialog, with automatic header detection
- **Multi-series support** — Plot multiple Y columns against a shared X axis
- **Multi-unit Y axes** — Normalized overlay when series have different units
- **ISO 8601 / RFC 3339 timestamps** — Automatic date format detection with subsecond precision
- **Interactive pan and zoom** — Click-and-drag to pan, scroll to zoom, auto-fit
- **Measurement cursors** — Vertical and horizontal cursor pairs with delta readout (per-unit for multi-axis)
- **LTTB downsampling** — Handles large datasets (100k+ points) without lag
- **Export** — Save as CSV, save as PNG, or copy image to clipboard
- **Drag-and-drop graph reordering** — Rearrange graphs by dragging the grip handle
- **Series z-index control** — Reorder series rendering priority in settings
- **Dynamic layout** — 1 graph fills the viewport, 2 split it in half, 3+ scroll
- **Theming** — Light and dark mode

## Building

Requires [Rust](https://rustup.rs/) (edition 2021).

```
cargo build --release
```

The release binary will be at `target/release/oxideplot.exe`.

## Running

```
cargo run --release
```

Or run the binary directly. Use **Add Data** to import a file, or drag-and-drop a `.csv` / `.xlsx` file onto the window.

## Dependencies

| Crate | Purpose |
|-------|---------|
| `eframe` / `egui` | UI framework with wgpu backend |
| `calamine` | Excel file reading |
| `csv` | CSV parsing |
| `chrono` | Timestamp parsing and formatting |
| `image` | PNG export |
| `arboard` | Clipboard image copy |
| `rfd` | Native file dialogs |
| `glam` / `bytemuck` | Math and GPU buffer types |

## License

All rights reserved.
