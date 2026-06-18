# Phase 8 — Math Functions / Transforms Design

**Date:** 2026-06-18
**Status:** Approved (user delegated design decisions) → implementation plan next
**Roadmap:** Phase 8 of `2026-06-18-oxideplot-feature-roadmap.md`

## Goal

Apply a math transform to an existing series, producing a **new derived series**
that appears in the series list and plots like any other series. Curated core
set; heavy analysis (FFT, filters, curve fitting) stays in backlog B3.

## Transforms (curated set)

Each operates on the source series' full `(xs, ys)` and returns a **new `ys`**
of the same length, paired with the **same `xs`**:

| Transform | Params | Semantics |
|-----------|--------|-----------|
| Moving average | `window: usize` (≥1, default 5) | **Centered** window mean: `y'[i]` = mean of `y[i-w/2 .. i+w/2]` clamped at the ends (uses available points near boundaries). |
| Derivative | none | `dy/dx`: central difference interior `(y[i+1]-y[i-1])/(x[i+1]-x[i-1])`; forward/backward at the two ends. |
| Integral | none | Cumulative trapezoidal: `Y[0]=0`, `Y[i]=Y[i-1]+(x[i]-x[i-1])*(y[i]+y[i-1])/2`. |
| Normalize | `mode: "minmax" \| "zscore"` (default minmax) | minmax → `(y-min)/(max-min)` (flat series → 0.5); zscore → `(y-mean)/std` (std 0 → 0). |
| Abs | none | `|y|` elementwise. |
| Log | none | `ln(y)` elementwise (non-positive → NaN; the existing finite-filter drops NaN points). |
| Sqrt | none | `sqrt(y)` elementwise (negative → NaN, dropped). |

These live in `crates/oxideplot-core/src/processing/math_ops.rs` (which already
exists) as pure functions, unit-tested natively.

## UX — per-series "fx" action

In `SeriesList.svelte`, each series row gets a small **"fx"** button. Clicking
opens an inline picker (popover or expanding row):
- a transform `<select>` (the 7 above);
- a contextual param input shown only when relevant — a number input for moving
  average's `window`, a `<select>` for normalize's `mode`; nothing for the rest;
- an **Apply** button.

Apply calls `renderer.addTransform(sourceIndex, kind, params)`. The new derived
series appears in the list immediately (refresh).

## Derived series

`add_transform` builds a new `SourceSeries` from the source's `xs` + the
transformed `ys`, with:
- **name** derived from the source + transform, e.g. `temp · MA(5)`,
  `d/dx(pressure)`, `∫(temp)`, `norm(temp)` / `z(temp)`, `|temp|`, `log(temp)`,
  `√(temp)`;
- a **distinct color** (cycle the existing palette by current series count);
- `draw_mode: Lines`, `visible: true`, and its own global `y_min/y_max`
  (computed like any source, for normalized mode).

A derived series is a **first-class series**: it shows in the series list and is
subject to visibility toggle, remove, reorder, draw-mode, and (importantly) can
itself be transformed again (chaining). It is **computed once** at add time from
a snapshot of the source's data (no live re-computation if the source is later
removed — removing a source does not remove its derivatives).

After adding, **auto-fit** so the new series is visible (derivatives/integrals
can have very different scales; the user has normalized mode + Fit + the series
list to manage scale/visibility).

Derived series are plot-only — they are NOT added to the loaded file's columns,
so they do not appear in the Phase 7 table (which shows the source file).

## WASM API (`OxidePlot`)

```
add_transform(source_index: usize, kind: String, params: JsValue) -> Result<(), JsValue>
```
- Bounds-check `source_index` against `sources.len()`.
- Read `sources[source_index]` `xs`/`ys`/`name`.
- Dispatch on `kind` (`"moving_average" | "derivative" | "integral" | "normalize" | "abs" | "log" | "sqrt"`); deserialize `params` into `{ window?: usize, mode?: String }` (a small `#[derive(Deserialize)]` struct); call the matching `math_ops` function.
- Build the derived `SourceSeries` (name, color, computed y_min/y_max), push it, `rebuild_visible()`, `auto_fit()` (which re-fits + renders).
- `renderer.ts`: `addTransform(sourceIndex, kind, params)` wrapper.

## Testing

Native unit tests in `math_ops.rs` for each transform on a small fixture:
- moving average (centered, window=3) smooths and clamps at edges;
- derivative of a linear ramp `y=2x` ≈ constant 2 (interior);
- integral of constant `y=1` over unit steps = cumulative `0,1,2,…`;
- normalize minmax maps to `[0,1]` with endpoints 0 and 1; zscore has ~mean 0;
- abs/log/sqrt elementwise (log of non-positive → NaN).

## Non-goals (backlog B3)

FFT, low/high/band-pass filters, detrend, curve fitting/regression, custom
formula expressions. This phase is the curated elementwise/windowed/calculus set.
