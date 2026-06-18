# Phase 9 — Interpolation / Resampling Design

**Date:** 2026-06-18
**Status:** Approved (user delegated design) → plan next
**Roadmap:** Phase 9 of `2026-06-18-oxideplot-feature-roadmap.md`

## Goal

Resample a series onto a uniform x-grid by interpolation (linear, nearest, or
natural cubic spline), producing a new derived series. Reuses Phase 8's
fx-picker + `add_transform` + derived-series machinery — this phase adds the
interpolation kernels (core), a `resample` transform kind (wasm), and the
"Resample" option in the fx picker (frontend).

## Behavior

- **Resample**: given the source series' `(xs, ys)`, generate `N` evenly-spaced
  x values over `[min(xs), max(xs)]` and compute y at each via the chosen method.
  Produces a derived series with the new `xs` (length N) and interpolated `ys`.
- **Methods:**
  - **linear** — piecewise-linear between bracketing source points.
  - **nearest** — value of the nearest source x.
  - **cubic** — natural cubic spline through the source points.
- **N** (point count): default 500, min 2. (Smaller N = decimate/smooth; larger
  N = upsample.)
- Assumes source `xs` are ascending (the standard time-series assumption used
  everywhere else). Non-finite source points are dropped before fitting.

## Core (`crates/oxideplot-core/src/processing/interpolation.rs`, new)

Pure, native-tested:
- `pub fn resample(xs: &[f64], ys: &[f64], n: usize, method: Method) -> (Vec<f64>, Vec<f64>)`
  where `pub enum Method { Linear, Nearest, Cubic }`. Builds the N-point uniform
  grid and evaluates the interpolant; returns `(grid_xs, interp_ys)`.
- Internal kernels: linear interp (binary-search bracket), nearest, and a
  natural cubic spline (solve the tridiagonal system for second derivatives,
  then evaluate per-segment). Degenerate inputs (< 2 finite points) return the
  input cloned (or empty) without panicking.

## WASM (`add_transform`)

- Extend `TransformParams` with `method: Option<String>` and `points: Option<usize>`.
- Add `kind == "resample"`: parse `method` ("linear"|"nearest"|"cubic", default
  linear) + `points` (default 500, min 2) → `interpolation::resample(...)`; build
  the derived `SourceSeries` with the NEW grid xs + interpolated ys (color +
  y_bounds via the same helpers; `auto_fit()` after). Label:
  `{base} · resample({method}, {n})`.
- `renderer.addTransform` already passes an arbitrary params object — no
  signature change; the frontend just sends `{ method, points }`.

## Frontend (fx picker in `SeriesList.svelte`)

- Add **"Resample"** (`resample`) to the transform `<select>`.
- When `resample` is selected, show two param controls: a **method** `<select>`
  (Linear / Nearest / Cubic spline → `linear`/`nearest`/`cubic`) and a **points**
  number input (default 500, min 2).
- On Apply with `resample`: `params = { method, points }`.

## Testing (native, in `interpolation.rs`)

- linear: resampling `y = 2x` (xs `[0,2,4]`) to a finer grid stays ≈ `2x` at the
  new points (e.g. x=1 → 2, x=3 → 6).
- nearest: a new x just past a source x picks that source's y.
- cubic: the spline passes through the original sample points (interpolating at a
  source x returns the source y within tolerance) and is smooth.
- resample count: output length == N; first/last grid x == min/max of source xs.
- degenerate: < 2 points returns without panic.

## Non-goals

NaN-aware gap filling, non-uniform target grids, spline variants beyond natural
cubic, extrapolation beyond the data range (grid is clamped to `[min,max]`).
