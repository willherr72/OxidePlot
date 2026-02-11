# OxidePlot Roadmap

A living document tracking the vision, planned features, and known issues for OxidePlot.

---

## 1. Core Plotting Engine

### Axis System
- [ ] Logarithmic axes (log10, ln, log2)
- [ ] Dual independent Y-axes (left/right) instead of normalized overlay
- [ ] Custom axis labels and titles (editable per-axis)
- [ ] Axis tick formatting options (scientific notation, engineering notation, SI prefixes)
- [ ] Inverted axes (flip direction)
- [ ] Time-aware X-axis ticks (smart labels like "14:30", "Feb 10", not raw timestamps)
- [ ] Date range selector (brush-select a time window)

### Plot Types
- [ ] Bar / histogram charts
- [ ] Box-and-whisker plots
- [ ] Heatmaps / colormaps
- [ ] Polar plots
- [ ] FFT / frequency spectrum view
- [ ] XY phase plots (Lissajous)
- [ ] Waterfall / stacked plots
- [ ] Fill-between (shade area between two series or between series and zero)
- [ ] Candlestick / OHLC (for financial or min/max/avg data)

### Series Rendering
- [ ] Marker shape selection (circle, square, triangle, cross, diamond)
- [ ] Dashed / dotted line styles
- [ ] Variable line width (data-driven thickness)
- [ ] Error bars (symmetric and asymmetric)
- [ ] Confidence bands / shaded regions
- [ ] Gradient-colored lines (color mapped to a third variable)
- [ ] Custom color palettes (viridis, plasma, inferno, user-defined)
- [ ] Colorblind-safe default palette

### Legend & Annotations
- [ ] Draggable legend positioning
- [ ] Legend outside plot area (top, bottom, right)
- [ ] Text annotations (click to place, editable)
- [ ] Arrow annotations (point to features)
- [ ] Horizontal/vertical reference lines (user-defined thresholds)
- [ ] Region highlighting (shade a range on X or Y)

---

## 2. File Format Support

### Import
- [ ] TSV / tab-separated values
- [ ] Parquet files (columnar, fast, common in data engineering)
- [ ] JSON / JSONL (array-of-objects and newline-delimited)
- [ ] HDF5 / NetCDF (scientific data)
- [ ] SQLite databases (query builder UI)
- [ ] Clipboard paste (tab-separated from Excel)
- [ ] URL fetch (load CSV from HTTP endpoint)
- [ ] Live file watching (tail -f style, auto-reload on file change)
- [ ] Multi-sheet Excel support (sheet selector in dialog)
- [ ] Encoding detection (UTF-8, UTF-16, Latin-1, Shift-JIS)
- [ ] Custom delimiter selection (semicolon, pipe, space)
- [ ] Skip rows / comment lines (# prefix)

### Export
- [ ] SVG export (vector, infinitely scalable)
- [ ] PDF export
- [ ] Excel export (.xlsx with charts)
- [ ] Parquet export
- [ ] LaTeX / TikZ export (for academic papers)
- [ ] High-DPI PNG export (configurable resolution)
- [ ] Export selected region only (crop to current view)
- [ ] Batch export (all graphs at once)
- [ ] Copy data to clipboard (tab-separated for Excel paste)

---

## 3. Data Processing & Analysis

### Transforms
- [ ] Moving average (simple, weighted, exponential)
- [ ] Low-pass / high-pass / band-pass filters (Butterworth, etc.)
- [ ] Derivative (dy/dx, numerical differentiation)
- [ ] Integral (cumulative sum / trapezoidal integration)
- [ ] FFT (Fast Fourier Transform with magnitude/phase output)
- [ ] Interpolation / resampling (linear, cubic spline, nearest)
- [ ] Decimate / downsample to specific sample rate
- [ ] Normalize (0-1 range, z-score, min-max)
- [ ] Detrend (remove linear/polynomial trend)
- [ ] Absolute value, log, sqrt, power transforms

### Curve Fitting & Regression
- [ ] Linear regression (with R-squared display)
- [ ] Polynomial regression (user-selectable degree)
- [ ] Exponential / logarithmic / power fits
- [ ] Custom equation fitting (user-provided formula)
- [ ] Display fit equation and parameters on plot
- [ ] Residual plot (show fit error as separate series)

### Statistics (expand existing)
- [ ] Percentiles (P25, P50, P75, P95, P99)
- [ ] RMS (root mean square)
- [ ] Skewness and kurtosis
- [ ] Correlation matrix between series
- [ ] Histogram of values (bin count, distribution shape)
- [ ] Outlier detection (IQR method, z-score method)
- [ ] Summary statistics export to clipboard/file

### Data Editing
- [ ] Manual point editing (click to move a data point)
- [ ] Delete selected points (lasso / rectangle selection)
- [ ] Crop data to visible range (discard points outside view)
- [ ] Merge series (combine two series into one)
- [ ] Split series at a point
- [ ] Rename columns after import
- [ ] Formula columns (computed from other columns, spreadsheet-style)

---

## 4. User Interface & Experience

### Keyboard Shortcuts
- [ ] Ctrl+O — Open file
- [ ] Ctrl+S — Save project
- [ ] Ctrl+Shift+S — Save project as
- [ ] Ctrl+Z / Ctrl+Y — Undo / Redo
- [ ] Ctrl+C — Copy visible plot to clipboard
- [ ] Ctrl+F — Find/filter in data table
- [ ] Del — Delete selected series or graph
- [ ] Home — Fit all data in view
- [ ] F11 — Fullscreen toggle
- [ ] Escape — Cancel current mode (cursor, etc.)
- [ ] 1/2/3 — Switch interpolation mode
- [ ] +/- — Zoom in/out
- [ ] Arrow keys — Pan

### Undo / Redo
- [ ] Command pattern for all destructive operations
- [ ] Undo stack with configurable depth
- [ ] Operations: add/remove series, add/remove graph, data transforms, axis changes

### Layout & Panels
- [ ] Resizable graph panels (drag border between graphs)
- [ ] Grid layout (2x2, 3x2, custom NxM arrangement)
- [ ] Tabbed graphs (switch between graphs without scrolling)
- [ ] Detachable graphs (pop out to separate window)
- [ ] Side panel for series list / data browser
- [ ] Minimap / overview bar for zoomed-in time series

### Data Table Improvements
- [ ] Virtual scrolling (handle millions of rows efficiently)
- [ ] Column resizing
- [ ] Column reordering
- [ ] Cell selection and copy
- [ ] Search / filter rows
- [ ] Freeze header row
- [ ] Conditional formatting (color cells by value)
- [ ] Edit cell values inline

### Theming & Appearance
- [ ] Custom themes (save/load color schemes)
- [ ] Per-graph background color
- [ ] Configurable font sizes
- [ ] Plot border styles (box, L-shaped, none)
- [ ] Grid style options (solid, dashed, dotted, none)
- [ ] Transparent background export (for presentations)

### Quality of Life
- [ ] Recent files list (persistent across sessions)
- [ ] Drag-and-drop series between graphs
- [ ] Right-click context menus on series, axes, legend items
- [ ] Tooltip improvements (show all series values at hover X, not just nearest)
- [ ] Crosshair mode (persistent crosshair follows mouse)
- [ ] Snap-to-data cursor mode
- [ ] Status bar showing cursor coordinates, zoom level, point count
- [ ] Progress bar for large file imports
- [ ] Welcome screen with recent projects and quick-start templates

---

## 5. CLI

### Basic Usage
```
oxideplot data.csv                      # Open file in GUI
oxideplot data.csv --columns time,temp  # Pre-select columns
oxideplot *.csv                         # Open multiple files
```

### Headless / Batch Mode
```
oxideplot render data.csv -o plot.png --width 1920 --height 1080
oxideplot render data.csv -o plot.svg --x time --y "temp,pressure"
oxideplot render data.csv -o plot.pdf --theme dark --title "Sensor Data"
```

### Data Inspection
```
oxideplot info data.csv                 # Print column names, types, row count
oxideplot stats data.csv --columns temp # Print statistics
oxideplot convert data.csv -o data.parquet
```

### Piping / Streaming
```
tail -f sensor.log | oxideplot --live   # Real-time streaming from stdin
curl https://api.example.com/data.csv | oxideplot -
```

### Project Files
```
oxideplot open project.oxideplot        # Open saved project
oxideplot export project.oxideplot --format png --all-graphs
```

---

## 6. Performance & Scalability

### Large Dataset Handling
- [ ] Streaming file parser (don't load entire file into RAM)
- [ ] Memory-mapped file access for huge datasets
- [ ] Level-of-detail rendering (coarser downsampling when zoomed out far)
- [ ] Background data loading with progress reporting
- [ ] Lazy column loading (only parse columns that are selected)
- [ ] Data compression in memory (delta encoding for timestamps)

### Rendering Performance
- [ ] Instanced rendering for large scatter plots
- [ ] Compute shader for downsampling on GPU
- [ ] Frame budget management (skip rendering if nothing changed)
- [ ] Dirty-rect optimization (only re-render changed regions)
- [ ] Shader pipeline caching across sessions
- [ ] Pre-allocated GPU buffers (avoid per-frame allocation)

### Benchmarking
- [ ] Built-in performance overlay (FPS, frame time, GPU memory)
- [ ] Benchmark suite with synthetic datasets (10K, 100K, 1M, 10M points)
- [ ] Memory profiling and leak detection

---

## 7. Testing & Reliability

### Unit Tests
- [ ] Date/time parsing (all formats, edge cases, timezones, DST)
- [ ] CSV parsing (quoted fields, embedded commas, empty cells, BOM)
- [ ] Excel parsing (merged cells, formulas, multiple sheets)
- [ ] LTTB downsampling correctness
- [ ] Math operations (edge cases: empty series, NaN, single point)
- [ ] Statistics computation accuracy
- [ ] Unit inference from column names
- [ ] Header detection heuristics
- [ ] f32 precision offset math

### Integration Tests
- [ ] Full file-load-to-render pipeline
- [ ] Project save → load round-trip
- [ ] Export → re-import round-trip
- [ ] Multi-graph synchronization
- [ ] Drag-and-drop reordering consistency

### Property-Based Tests
- [ ] Arbitrary CSV data doesn't crash the parser
- [ ] Random zoom/pan sequences don't cause NaN/Inf in view state
- [ ] Downsampled data preserves min/max of original

### Test Data
- [ ] Sample CSV files covering edge cases
- [ ] Sample Excel files (multi-sheet, dates, mixed types)
- [ ] Large generated datasets for performance testing
- [ ] Known-good reference images for rendering regression tests

---

## 8. Code Quality & Architecture

### Refactoring
- [ ] Extract magic numbers into named constants
- [ ] Split graph_panel.rs into smaller modules (toolbar, gpu_2d, gpu_3d, table, cursors)
- [ ] Replace unwrap() calls with proper error propagation
- [ ] Add doc comments to all public types and functions
- [ ] Activate the unused KD-tree module or remove it
- [ ] Remove unused struct fields (downsampled_x/y, etc.)

### Error Handling
- [ ] Structured error types (thiserror or anyhow)
- [ ] User-facing error messages for all failure modes
- [ ] Error recovery (don't lose state on a failed operation)
- [ ] Crash reporting / panic handler with state dump

### Architecture
- [ ] Event system (decouple UI from data model)
- [ ] Plugin / extension API (custom data sources, transforms, renderers)
- [ ] Separate data model from view state cleanly
- [ ] Consider ECS-like pattern for series/graph management

---

## 9. Platform & Distribution

### Cross-Platform
- [ ] Linux build and test
- [ ] macOS build and test (Metal backend)
- [ ] Verify Vulkan fallback on Windows
- [ ] Verify GL fallback works

### Packaging
- [ ] Windows installer (MSI or NSIS)
- [ ] Windows portable .zip release
- [ ] File association (.csv, .oxideplot double-click opens app)
- [ ] App icon and branding
- [ ] Winget / Scoop / Chocolatey package
- [ ] GitHub Actions CI/CD (build + test on push, release binaries on tag)

### Distribution
- [ ] GitHub Releases with pre-built binaries
- [ ] Cargo install support (publish to crates.io)
- [ ] Homebrew formula (macOS)
- [ ] Flatpak / AppImage (Linux)

---

## 10. Advanced / Stretch Goals

### Real-Time Data
- [ ] Live data feed (serial port / COM port input)
- [ ] MQTT / WebSocket data source
- [ ] Rolling window mode (fixed-width sliding view)
- [ ] Trigger / capture mode (start recording on threshold)
- [ ] Data rate display (samples/sec)

### Collaboration
- [ ] Shared project files via link
- [ ] Export shareable HTML (self-contained interactive chart)
- [ ] Embed plot in Markdown / Jupyter notebooks

### Scripting
- [ ] Embedded scripting engine (Rhai or Lua) for custom transforms
- [ ] Macro recorder (record UI actions, replay as script)
- [ ] Template system (save a plot configuration, apply to new data)

### AI/ML Integration
- [ ] Anomaly detection highlighting
- [ ] Automatic trend description
- [ ] Smart column type detection (beyond numeric/datetime)
- [ ] Suggested visualizations based on data shape

---

## Known Bugs & Technical Debt

- [ ] `lock().unwrap()` calls can panic on poisoned mutex (3 instances in app.rs)
- [ ] `as_ref().unwrap()` in settings dialog stats cache
- [ ] No overflow protection on atomic graph/series ID counters
- [ ] Unsorted data causes O(n) scan in downsampler instead of O(log n)
- [ ] Table view renders all rows (no virtualization, slow for large datasets)
- [ ] Grid lines and normalization recomputed every frame (could cache)
- [ ] 12-color palette cycles — collisions with >12 series
- [ ] Timestamp error correction threshold (2001) is arbitrary and undocumented
- [ ] No maximum limits on graphs, series, or points (OOM possible)
- [ ] `graphie.py` file in repo root appears to be leftover from Python prototype — remove or document
