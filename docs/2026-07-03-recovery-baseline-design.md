# Recovery Baselines for Trip Detail — Design

Date: 2026-07-03
Status: approved
Depends on: Phase 2 (trip detail endpoint + recovery charts, shipped in `hiking-phase2`)

## Goal

The trip detail recovery charts show absolute values during the trip with nothing to
compare against. Add a per-trip **baseline** (the user's normal at-home level per
metric), widen the recovery window to show pre-trip normal → during-trip dip →
post-trip bounce-back, and compute a plain-language **recovery verdict** per trip.

This answers two user questions directly:

- *Was I overreaching mid-trip?* — metrics visibly drifting away from the baseline line.
- *How did the trip affect me?* — peak deviation and days-to-recover after the trip.

## Data feasibility (verified against production garmin.db, 2026-07-03)

| Metric | Coverage |
|---|---|
| Sleep score & duration | 2017 → now (138/155 PCT 2022 nights) |
| Resting HR | 2017 → now, full PCT coverage |
| Body battery, stress | full coverage incl. PCT |
| HRV (`hrv.last_night_avg`) | 2022-10-06 → now |
| Training readiness (`training_readiness.score`) | 2022-10-06 → now, 1361 days |
| VO2max | table empty — excluded |

Metrics absent for a period (e.g. HRV during PCT) degrade gracefully: charts and
verdict entries are simply omitted.

## 1. Baseline computation (engine)

New pure module `src-tauri/src/hiking/baseline.rs`.

- For each recovery metric, **baseline = median** of daily values over the **28 days
  before trip start**, excluding any day that falls within any trip's date range
  (from the effective post-override trip list — otherwise back-to-back trips poison
  each other's baselines).
- If fewer than **7 usable days with data** for a metric, widen the window to **56
  days** (same exclusions). If still fewer than 7, that metric's baseline is `null`.
- Median, not mean: one outlier night must not skew "normal".

Signature (approximate):

```rust
fn baselines(days: &[RecoveryDay], trip_ranges: &[(NaiveDate, NaiveDate)], trip_start: NaiveDate) -> Baselines
```

`Baselines` holds one `Option<f64>` per metric: `sleep_score`, `sleep_seconds`,
`resting_hr`, `hrv_last_night_avg`, `body_battery_high`, `body_battery_low`,
`avg_stress`, `training_readiness`.

## 2. Recovery window + verdict (engine)

- The `recovery` series in the trip detail payload expands from trip-days-only to
  **trip start − 14 days → trip end + 14 days**.
- New pure function computes a per-trip **recovery summary** for the two most honest
  strain signals, **resting HR** and **HRV**:
  - **Peak deviation**: the signed value `metric − baseline` at the worst point
    during the trip (positive for RHR, which strains upward; negative for HRV,
    which strains downward), with the trip day it occurred on (day 1 = first
    trip day).
  - **Days-to-recover**: first day after trip end where the metric is back within
    tolerance for **2 consecutive days**. Tolerances: RHR ≤ baseline + 2 bpm;
    HRV ≥ baseline × 0.95. Search capped at 21 days after trip end.
- Per-metric summary is `null` when the baseline is `null` or there is no during-trip
  data for that metric (UI omits it). Within a non-null summary,
  `days_to_recover: null` means *not recovered within 21 days* (UI shows "21+ days").

## 3. Data layer + API

- `store.rs`: fetch the wider date range (start − 56 days for baselines, end + 21
  days for recovery detection; the payload only carries ±14 around the trip). Add one
  new metric to `RecoveryDay`: `training_readiness: Option<i64>` from
  `training_readiness.score`.
- `/api/hiking/trips/{id}` payload — additive changes only:

```jsonc
{
  // existing fields unchanged
  "recovery": [ /* RecoveryDay, now start-14 .. end+14, + training_readiness */ ],
  "baselines": { "resting_hr": 52.0, "hrv_last_night_avg": null, /* ... one per metric */ },
  "recovery_summary": {
    "resting_hr": { "peak_deviation": 6.0, "peak_trip_day": 12, "days_to_recover": 4 },
    "hrv": null   // e.g. pre-2022 trip
  }
}
```

## 4. Frontend (trip detail)

- Each recovery chart gains a **dashed horizontal baseline line** (ECharts
  `markLine`) and a **shaded band over the trip days** (`markArea`), so
  pre/during/post reads at a glance. Trip band derives from the trip's start/end
  dates already in the payload.
- A **fifth chart, training readiness**, renders only when the series has data.
- A short **verdict block** above the charts, e.g.:
  *"Resting HR peaked +6 bpm over baseline (day 12) · back to baseline 4 days after
  the trip · HRV: recovered in 2 days."* Metrics with a null summary are omitted;
  `days_to_recover: null` renders as "not back to baseline within 21 days".
- A 155-day trip plus the ±14-day window is ~183 points — no chart performance
  concern.

## 5. Testing

- `baseline.rs` and the recovery-summary function: **strict TDD** with synthetic
  fixtures — sparse data (28→56 widening, then null), back-to-back trip exclusion,
  missing HRV (pre-2022), never-recovers (21+ cap), recovery requiring the
  2-consecutive-day rule.
- Store query widening: extend the existing `--ignored` real-DB tests
  (`GARMIN_DB_TEST=/tmp/garmin.db`) with an assertion on window width and the new
  training_readiness field.
- Frontend: `npx tsc --noEmit` + `npm run build`; visual check on live data against
  PCT 2022 (long trip, no HRV) and a recent short trip (full metrics).
- Existing gates stay green: `cargo test --features web`, `cargo check --features
  tauri-app` (any new HTTP wiring stays behind the web-only cfg gate).

## Explicit non-goals (deferred until A proves itself)

- **B. Long-term recovery trends view** (one data point per trip across years).
- **C. Sustainable-load insight** (load vs. degradation correlation).
- Badges/verdicts on the trip list; only trip detail changes.

## Chosen defaults (overridable later)

- ±14-day display window (not 7 or 21).
- Baseline window 28 days, widening to 56, minimum 7 usable days.
- Recovery tolerances: RHR +2 bpm, HRV −5%, 2 consecutive days, 21-day cap.
