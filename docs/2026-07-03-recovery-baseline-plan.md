# Recovery Baselines Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-trip recovery baselines, a ±14-day recovery window, and a plain-language recovery verdict (peak deviation + days-to-recover for resting HR and HRV) to the hiking trip detail view.

**Architecture:** A new pure module `src-tauri/src/hiking/baseline.rs` computes median baselines from pre-trip days and the recovery summary; `http.rs` wires it into the existing `/api/hiking/trips/{id}` handler (additive payload fields); the frontend adds baseline reference lines, a trip-period band, and a verdict block to the existing recovery charts.

**Tech Stack:** Rust (chrono, serde, rusqlite via existing store), axum handler (already cfg-gated web-only), React + echarts-for-react.

**Spec:** `docs/2026-07-03-recovery-baseline-design.md` (approved). Note: the spec's "add training_readiness to store + a fifth chart" is **already shipped** — `store.rs` fetches it and `HikingTripDetail.tsx` renders the chart. Remaining work there is only: render that chart conditionally on data existing.

**Conventions that bind every task:**
- Engine code in `src-tauri/src/hiking/` is pure (no `crate::server`, no cfg gate needed) and strict TDD.
- Anything importing `crate::server` must sit behind `#[cfg(all(feature = "web", not(feature = "tauri-app")))]` — `http.rs` already is; do not add new ungated server imports.
- Run cargo with `export PATH="$HOME/.cargo/bin:$PATH"` and `--manifest-path src-tauri/Cargo.toml`.
- All commits end with the trailer shown in each commit step.

**File structure (whole feature):**
- Create: `src-tauri/src/hiking/baseline.rs` — `Baselines`, `baselines()`, `MetricSummary`, `RecoverySummary`, `recovery_summary()` + unit tests
- Modify: `src-tauri/src/hiking/mod.rs` — register module
- Modify: `src-tauri/src/hiking/http.rs` — widen recovery fetch, compute baselines/summary, extend `TripDetail`
- Modify: `src-tauri/src/hiking/store.rs` — one new `--ignored` real-DB test
- Modify: `src/types.ts` — `Baselines`, `MetricSummary`, `RecoverySummary`, extend `TripDetail`
- Modify: `src/components/HikingTripDetail.tsx` — chart baseline/markArea, conditional readiness chart, verdict block
- Modify: `src/styles.css` — `.hiking-verdict` rule (near `.hiking-superlatives`, ~line 3285)

---

### Task 1: Baseline computation (`baselines()`)

**Files:**
- Create: `src-tauri/src/hiking/baseline.rs`
- Modify: `src-tauri/src/hiking/mod.rs` (add `pub mod baseline;` after `pub mod store;`)

Baseline = median of a metric over the 28 days before trip start, excluding days inside any trip's date range; widen to 56 days if fewer than 7 usable values; `None` if still fewer than 7. Median of an even count = mean of the two middle values.

- [ ] **Step 1: Register the module and write the failing tests**

In `src-tauri/src/hiking/mod.rs`, after `pub mod store;` add:

```rust
pub mod baseline;
```

Create `src-tauri/src/hiking/baseline.rs`:

```rust
use chrono::{Duration, NaiveDate};
use crate::hiking::store::RecoveryDay;

/// Per-metric at-home baseline (median of pre-trip days). None = insufficient data.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Baselines {
    pub sleep_score: Option<f64>,
    pub sleep_seconds: Option<f64>,
    pub resting_hr: Option<f64>,
    pub hrv_last_night_avg: Option<f64>,
    pub body_battery_high: Option<f64>,
    pub body_battery_low: Option<f64>,
    pub avg_stress: Option<f64>,
    pub training_readiness: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// RecoveryDay fixture with only the fields these tests use.
    fn day(date: &str, rhr: Option<i64>, hrv: Option<f64>) -> RecoveryDay {
        RecoveryDay {
            date: date.into(),
            sleep_score: None,
            sleep_seconds: None,
            resting_hr: rhr,
            hrv_last_night_avg: hrv,
            body_battery_high: None,
            body_battery_low: None,
            avg_stress: None,
            training_readiness: None,
        }
    }

    #[test]
    fn median_of_28_day_window() {
        // 7 usable days, values 50..=56 -> median 53
        let days: Vec<RecoveryDay> = (0..7)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(53.0));
        assert_eq!(b.hrv_last_night_avg, None); // no HRV data at all
    }

    #[test]
    fn even_count_averages_middle_two() {
        // 8 values 50..=57 -> (53 + 54) / 2 = 53.5
        let days: Vec<RecoveryDay> = (0..8)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(53.5));
    }

    #[test]
    fn excludes_days_inside_trips() {
        // 7 clean days (median 53) + 3 poisoned days inside a prior trip
        let mut days: Vec<RecoveryDay> = (0..7)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        for i in 20..=22 {
            days.push(day(&format!("2024-05-{i}"), Some(90), None));
        }
        let trips = [(d("2024-05-20"), d("2024-05-22"))];
        let b = baselines(&days, &trips, d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(53.0));
    }

    #[test]
    fn widens_to_56_days_when_sparse() {
        // Only 5 values inside the 28-day window; 4 more in days -56..-29.
        // 9 values total: [50,51,52,53,54,60,61,62,63] -> median 54
        let mut days: Vec<RecoveryDay> = (0..5)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        for i in 0..4 {
            days.push(day(&format!("2024-04-{:02}", 10 + i), Some(60 + i), None));
        }
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(54.0));
    }

    #[test]
    fn insufficient_data_gives_none() {
        // 6 values even in the 56-day window -> None
        let days: Vec<RecoveryDay> = (0..6)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile (no `baselines` fn yet)**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --features web --manifest-path src-tauri/Cargo.toml baseline::
```

Expected: compile error `cannot find function `baselines` in this scope`.

- [ ] **Step 3: Implement `baselines()`**

Add above the `#[cfg(test)]` block in `baseline.rs`:

```rust
fn parse_date(s: &str) -> NaiveDate {
    // dates come from store::load_recovery, always ISO
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("ISO date from store")
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Median over the 28 days before trip_start (widened to 56 if < 7 usable
/// values), skipping days inside any trip range. None if still < 7 values.
fn metric_baseline(
    days: &[RecoveryDay],
    trip_ranges: &[(NaiveDate, NaiveDate)],
    trip_start: NaiveDate,
    get: &dyn Fn(&RecoveryDay) -> Option<f64>,
) -> Option<f64> {
    for window in [28i64, 56] {
        let from = trip_start - Duration::days(window);
        let vals: Vec<f64> = days
            .iter()
            .filter(|d| {
                let dt = parse_date(&d.date);
                dt >= from
                    && dt < trip_start
                    && !trip_ranges.iter().any(|(s, e)| dt >= *s && dt <= *e)
            })
            .filter_map(get)
            .collect();
        if vals.len() >= 7 {
            return median(vals);
        }
    }
    None
}

pub fn baselines(
    days: &[RecoveryDay],
    trip_ranges: &[(NaiveDate, NaiveDate)],
    trip_start: NaiveDate,
) -> Baselines {
    let f = |get: &dyn Fn(&RecoveryDay) -> Option<f64>| {
        metric_baseline(days, trip_ranges, trip_start, get)
    };
    Baselines {
        sleep_score: f(&|d| d.sleep_score.map(|v| v as f64)),
        sleep_seconds: f(&|d| d.sleep_seconds.map(|v| v as f64)),
        resting_hr: f(&|d| d.resting_hr.map(|v| v as f64)),
        hrv_last_night_avg: f(&|d| d.hrv_last_night_avg),
        body_battery_high: f(&|d| d.body_battery_high.map(|v| v as f64)),
        body_battery_low: f(&|d| d.body_battery_low.map(|v| v as f64)),
        avg_stress: f(&|d| d.avg_stress.map(|v| v as f64)),
        training_readiness: f(&|d| d.training_readiness),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test --features web --manifest-path src-tauri/Cargo.toml baseline::
```

Expected: `test result: ok. 5 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/hiking/baseline.rs src-tauri/src/hiking/mod.rs
git commit -m "feat(hiking): per-trip recovery baselines (median of pre-trip days)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Recovery summary (`recovery_summary()`)

**Files:**
- Modify: `src-tauri/src/hiking/baseline.rs`

For resting HR (strains upward, recovered when `v <= baseline + 2.0`) and HRV (strains downward, recovered when `v >= baseline * 0.95`): peak signed deviation `value − baseline` during trip days (with 1-based trip day), and days-to-recover = first `i` in `1..=21` where days `end+i` **and** `end+i+1` both have data and are within tolerance. `None` per metric when baseline is `None` or no during-trip data; `days_to_recover: None` means not recovered within 21 days.

- [ ] **Step 1: Write the failing tests**

Add inside the existing `mod tests` in `baseline.rs`. All use trip `2024-06-01 .. 2024-06-03` and 7 pre-trip days establishing the baseline.

```rust
    /// 7 pre-trip days: rhr 50, hrv 60 -> baselines rhr=50.0, hrv=60.0
    fn pre_days() -> Vec<RecoveryDay> {
        (0..7)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50), Some(60.0)))
            .collect()
    }

    fn trip() -> (NaiveDate, NaiveDate) {
        (d("2024-06-01"), d("2024-06-03"))
    }

    #[test]
    fn rhr_peak_deviation_and_day() {
        let mut days = pre_days();
        days.push(day("2024-06-01", Some(55), None));
        days.push(day("2024-06-02", Some(58), None));
        days.push(day("2024-06-03", Some(56), None));
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        let rhr = s.resting_hr.expect("rhr summary");
        assert_eq!(rhr.peak_deviation, 8.0);
        assert_eq!(rhr.peak_trip_day, 2);
    }

    #[test]
    fn days_to_recover_needs_two_consecutive_days() {
        // tolerance: <= 50 + 2 = 52
        let mut days = pre_days();
        days.push(day("2024-06-02", Some(58), None));
        days.push(day("2024-06-04", Some(55), None)); // +1: no
        days.push(day("2024-06-05", Some(52), None)); // +2: ok, but +3 not ok
        days.push(day("2024-06-06", Some(54), None)); // +3: no
        days.push(day("2024-06-07", Some(51), None)); // +4: ok
        days.push(day("2024-06-08", Some(52), None)); // +5: ok -> recovered at 4
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        assert_eq!(s.resting_hr.expect("rhr summary").days_to_recover, Some(4));
    }

    #[test]
    fn not_recovered_within_21_days_is_none() {
        let mut days = pre_days();
        days.push(day("2024-06-02", Some(58), None));
        for i in 4..=26 {
            days.push(day(&format!("2024-06-{i:02}"), Some(60), None));
        }
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        assert_eq!(s.resting_hr.expect("rhr summary").days_to_recover, None);
    }

    #[test]
    fn hrv_strains_downward() {
        // baseline 60, tolerance >= 57 (60 * 0.95)
        let mut days = pre_days();
        days.push(day("2024-06-01", None, Some(55.0)));
        days.push(day("2024-06-02", None, Some(50.0)));
        days.push(day("2024-06-03", None, Some(57.0)));
        days.push(day("2024-06-04", None, Some(57.0))); // +1: ok
        days.push(day("2024-06-05", None, Some(58.0))); // +2: ok -> recovered at 1
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        let hrv = s.hrv.expect("hrv summary");
        assert_eq!(hrv.peak_deviation, -10.0);
        assert_eq!(hrv.peak_trip_day, 2);
        assert_eq!(hrv.days_to_recover, Some(1));
    }

    #[test]
    fn no_baseline_means_no_summary() {
        // no pre-trip data at all
        let days = vec![day("2024-06-02", Some(58), Some(50.0))];
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        assert!(s.resting_hr.is_none());
        assert!(s.hrv.is_none());
    }

    #[test]
    fn no_during_trip_data_means_no_summary() {
        let days = pre_days(); // baseline exists, but no trip-day values
        let (start, end) = trip();
        let b = baselines(&days, &[], start);
        let s = recovery_summary(&days, &b, start, end);
        assert!(s.resting_hr.is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail to compile**

```bash
cargo test --features web --manifest-path src-tauri/Cargo.toml baseline::
```

Expected: compile error `cannot find function `recovery_summary``.

- [ ] **Step 3: Implement `recovery_summary()`**

Add above the `#[cfg(test)]` block:

```rust
/// Strain + bounce-back for one metric. peak_deviation is signed
/// (value - baseline): positive for RHR, negative for HRV.
/// days_to_recover: None = not back within 21 days of trip end.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricSummary {
    pub peak_deviation: f64,
    pub peak_trip_day: i64, // 1 = first trip day
    pub days_to_recover: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoverySummary {
    pub resting_hr: Option<MetricSummary>,
    pub hrv: Option<MetricSummary>,
}

fn metric_summary(
    days: &[RecoveryDay],
    baseline: Option<f64>,
    trip_start: NaiveDate,
    trip_end: NaiveDate,
    get: &dyn Fn(&RecoveryDay) -> Option<f64>,
    strains_up: bool,
    recovered: &dyn Fn(f64, f64) -> bool,
) -> Option<MetricSummary> {
    let baseline = baseline?;
    let mut peak: Option<(f64, i64)> = None; // (deviation, trip day)
    for rd in days {
        let dt = parse_date(&rd.date);
        if dt < trip_start || dt > trip_end {
            continue;
        }
        let Some(v) = get(rd) else { continue };
        let dev = v - baseline;
        let worse = match peak {
            None => true,
            Some((p, _)) => if strains_up { dev > p } else { dev < p },
        };
        if worse {
            peak = Some((dev, (dt - trip_start).num_days() + 1));
        }
    }
    let (peak_deviation, peak_trip_day) = peak?;

    let value_on = |date: NaiveDate| -> Option<f64> {
        days.iter().find(|rd| parse_date(&rd.date) == date).and_then(get)
    };
    let ok = |date: NaiveDate| value_on(date).map(|v| recovered(v, baseline)).unwrap_or(false);
    let days_to_recover = (1..=21).find(|&i| {
        let d0 = trip_end + Duration::days(i);
        ok(d0) && ok(d0 + Duration::days(1))
    });
    Some(MetricSummary { peak_deviation, peak_trip_day, days_to_recover })
}

pub fn recovery_summary(
    days: &[RecoveryDay],
    baselines: &Baselines,
    trip_start: NaiveDate,
    trip_end: NaiveDate,
) -> RecoverySummary {
    RecoverySummary {
        resting_hr: metric_summary(
            days, baselines.resting_hr, trip_start, trip_end,
            &|d| d.resting_hr.map(|v| v as f64), true,
            &|v, b| v <= b + 2.0,
        ),
        hrv: metric_summary(
            days, baselines.hrv_last_night_avg, trip_start, trip_end,
            &|d| d.hrv_last_night_avg, false,
            &|v, b| v >= b * 0.95,
        ),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test --features web --manifest-path src-tauri/Cargo.toml baseline::
```

Expected: `test result: ok. 11 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/hiking/baseline.rs
git commit -m "feat(hiking): recovery summary (peak deviation + days-to-recover)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Wire into the trip detail endpoint

**Files:**
- Modify: `src-tauri/src/hiking/http.rs` (imports line 9, `TripDetail` struct ~line 126, `hiking_trip_detail` handler ~line 135)
- Modify: `src-tauri/src/hiking/store.rs` (one new `--ignored` test)

Fetch `start − 56 .. end + 22` (56 pre-days for baselines; 22 post-days so the 2-consecutive check at day 21 has data), compute baselines + summary against all trips' date ranges, and slice the payload's `recovery` to `start − 14 .. end + 14`.

- [ ] **Step 1: Extend `TripDetail` and the handler**

In `src-tauri/src/hiking/http.rs` line 9, add `baseline`:

```rust
use crate::hiking::{HikingSettings, Override, store, cluster, aggregate, baseline};
```

Add two fields to `TripDetail` (after `pub recovery: Vec<store::RecoveryDay>,`):

```rust
    pub baselines: baseline::Baselines,
    pub recovery_summary: baseline::RecoverySummary,
```

In `hiking_trip_detail`, replace:

```rust
    let recovery = store::load_recovery(&garmin_db_path, trip.start_date, trip.end_date).map_err(|e| {
        tracing::error!(error = %e, "garmin.db recovery load failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
```

with:

```rust
    // 56 pre-days feed the baseline; 22 post-days let the 2-consecutive
    // recovery check reach day 21. The payload only carries +/-14 days.
    let fetch_start = trip.start_date - chrono::Duration::days(56);
    let fetch_end = trip.end_date + chrono::Duration::days(22);
    let all_recovery = store::load_recovery(&garmin_db_path, fetch_start, fetch_end).map_err(|e| {
        tracing::error!(error = %e, "garmin.db recovery load failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let trip_ranges: Vec<(chrono::NaiveDate, chrono::NaiveDate)> =
        trips.iter().map(|t| (t.start_date, t.end_date)).collect();
    let baselines = baseline::baselines(&all_recovery, &trip_ranges, trip.start_date);
    let recovery_summary =
        baseline::recovery_summary(&all_recovery, &baselines, trip.start_date, trip.end_date);
    let win_start = (trip.start_date - chrono::Duration::days(14)).to_string();
    let win_end = (trip.end_date + chrono::Duration::days(14)).to_string();
    let recovery: Vec<store::RecoveryDay> = all_recovery
        .into_iter()
        .filter(|d| d.date.as_str() >= win_start.as_str() && d.date.as_str() <= win_end.as_str())
        .collect();
```

Add the two new fields to the `Ok(Json(TripDetail { ... }))` literal:

```rust
    Ok(Json(TripDetail {
        merged,
        has_previous: idx > 0,
        superlatives,
        days,
        recovery,
        baselines,
        recovery_summary,
        trip,
    }))
```

- [ ] **Step 2: Verify both feature builds**

```bash
cargo test --features web --manifest-path src-tauri/Cargo.toml hiking
cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml
```

Expected: all hiking tests pass (25 existing + 11 new = 36 passed, 2 ignored); tauri check clean (`baseline.rs` is pure, `http.rs` stays behind the existing web cfg gate).

- [ ] **Step 3: Add the real-DB training_readiness test**

In `src-tauri/src/hiking/store.rs`, inside `mod tests` after `loads_recovery_from_real_db`:

```rust
    #[test]
    #[ignore] // needs a real garmin.db at GARMIN_DB_TEST
    fn loads_training_readiness_from_real_db() {
        // readiness data exists from 2022-10-06 onward
        let p = std::env::var("GARMIN_DB_TEST").expect("set GARMIN_DB_TEST");
        let days = load_recovery(
            std::path::Path::new(&p),
            chrono::NaiveDate::from_ymd_opt(2023, 6, 1).unwrap(),
            chrono::NaiveDate::from_ymd_opt(2023, 6, 10).unwrap(),
        ).unwrap();
        assert!(days.iter().any(|d| d.training_readiness.is_some()));
    }
```

- [ ] **Step 4: Run the ignored real-DB tests**

Needs `/tmp/garmin.db` (re-scp from `root@arvi:/opt/appdata/fitness/givemydata/garmin.db` if evicted).

```bash
GARMIN_DB_TEST=/tmp/garmin.db cargo test --features web --manifest-path src-tauri/Cargo.toml store:: -- --ignored
```

Expected: `3 passed`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/hiking/http.rs src-tauri/src/hiking/store.rs
git commit -m "feat(hiking): trip detail carries baselines, recovery summary, +/-14d window

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Frontend types + chart upgrades

**Files:**
- Modify: `src/types.ts` (after the `RecoveryDay` type, ~line 83; `TripDetail` ~line 91)
- Modify: `src/components/HikingTripDetail.tsx`

- [ ] **Step 1: Add the types**

In `src/types.ts`, after the `RecoveryDay` type:

```ts
export type Baselines = {
  sleep_score: number | null;
  sleep_seconds: number | null;
  resting_hr: number | null;
  hrv_last_night_avg: number | null;
  body_battery_high: number | null;
  body_battery_low: number | null;
  avg_stress: number | null;
  training_readiness: number | null;
};

export type MetricSummary = {
  peak_deviation: number;
  peak_trip_day: number;
  days_to_recover: number | null;
};

export type RecoverySummary = {
  resting_hr: MetricSummary | null;
  hrv: MetricSummary | null;
};
```

Extend `TripDetail` with two fields:

```ts
  baselines: Baselines;
  recovery_summary: RecoverySummary;
```

- [ ] **Step 2: Upgrade `recoveryChart` with baseline lines and the trip band**

In `src/components/HikingTripDetail.tsx`:

Import the new type (line 4):

```ts
import type { Baselines, RecordPoint, RecoveryDay, TripDetail } from "../types";
```

Add a band color to `ChartColors` (type and the `chartColors` literal):

```ts
type ChartColors = {
  axisColor: string;
  tooltipBg: string;
  tooltipBorder: string;
  tooltipText: string;
  tripBand: string;
};
```

```ts
    tripBand: isDark ? "rgba(100, 140, 220, 0.10)" : "rgba(59, 130, 246, 0.08)",
```

Replace the whole `recoveryChart` function (title/tooltip/grid/xAxis/yAxis/legend are unchanged from the current file; the signature and `series` mapping are new):

```ts
function recoveryChart(
  recovery: RecoveryDay[],
  series: Array<{ key: keyof RecoveryDay; label: string }>,
  title: string,
  colors: ChartColors,
  baselines: Baselines,
  tripStart: string,
  tripEnd: string,
) {
  const startIdx = recovery.findIndex((r) => r.date === tripStart);
  const endIdx = recovery.findIndex((r) => r.date === tripEnd);
  return {
    title: { text: title, textStyle: { fontSize: 12, color: colors.axisColor }, left: 4, top: 2 },
    tooltip: {
      trigger: "axis",
      backgroundColor: colors.tooltipBg,
      borderColor: colors.tooltipBorder,
      textStyle: { color: colors.tooltipText },
    },
    grid: { left: 36, right: 12, top: 30, bottom: 20 },
    xAxis: {
      type: "category",
      data: recovery.map((r) => r.date.slice(5)),
      axisLabel: { fontSize: 9, color: colors.axisColor },
    },
    yAxis: { type: "value", axisLabel: { fontSize: 9, color: colors.axisColor }, scale: true },
    series: series.map((s, i) => {
      const base = baselines[s.key as keyof Baselines];
      return {
        name: s.label,
        type: "line",
        connectNulls: true,
        showSymbol: false,
        data: recovery.map((r) => r[s.key] as number | null),
        // dashed at-home baseline for this metric
        markLine:
          base != null
            ? {
                silent: true,
                symbol: "none",
                lineStyle: { type: "dashed", opacity: 0.6 },
                label: { show: false },
                data: [{ yAxis: base }],
              }
            : undefined,
        // shaded band over the trip days (first series only; +/-0.5 covers
        // the full category slots so single-day trips still show a band)
        markArea:
          i === 0 && startIdx >= 0 && endIdx >= 0
            ? {
                silent: true,
                itemStyle: { color: colors.tripBand },
                data: [[{ xAxis: startIdx - 0.5 }, { xAxis: endIdx + 0.5 }]],
              }
            : undefined,
      };
    }),
    legend:
      series.length > 1
        ? { bottom: 0, textStyle: { fontSize: 9, color: colors.axisColor } }
        : undefined,
  };
}
```

(`s.key as keyof Baselines` is safe: every series key used is a metric key, never `date`.)

Update all four call sites to pass the new arguments, e.g. the first one:

```ts
            option={recoveryChart(
              detail.recovery,
              [{ key: "sleep_score", label: "Sleep score" }],
              "Sleep score",
              chartColors,
              detail.baselines,
              t.start_date,
              t.end_date,
            )}
```

(same three extra arguments for the other three charts).

- [ ] **Step 3: Render the training-readiness panel only when data exists**

Wrap the fourth chart's `<div className="panel">` in:

```tsx
        {detail.recovery.some((r) => r.training_readiness != null) && (
          <div className="panel">
            ...existing readiness ReactECharts...
          </div>
        )}
```

- [ ] **Step 4: Type-check and build**

```bash
npx tsc --noEmit && npm run build
```

Expected: both clean.

- [ ] **Step 5: Commit**

```bash
git add src/types.ts src/components/HikingTripDetail.tsx
git commit -m "feat(hiking): baseline lines + trip band on recovery charts

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Verdict block

**Files:**
- Modify: `src/components/HikingTripDetail.tsx`
- Modify: `src/styles.css` (next to `.hiking-superlatives`, ~line 3285)

- [ ] **Step 1: Add the verdict helper and block**

In `HikingTripDetail.tsx`, extend the type import with `MetricSummary`:

```ts
import type { Baselines, MetricSummary, RecordPoint, RecoveryDay, TripDetail } from "../types";
```

Add next to `recoveryChart` (module level):

```ts
function metricVerdict(label: string, m: MetricSummary, unit: string): string {
  const sign = m.peak_deviation >= 0 ? "+" : "";
  const recov =
    m.days_to_recover != null
      ? `back to baseline ${m.days_to_recover} day${m.days_to_recover === 1 ? "" : "s"} after the trip`
      : "not back to baseline within 21 days";
  return `${label} peaked ${sign}${m.peak_deviation.toFixed(0)} ${unit} vs baseline (day ${m.peak_trip_day}) · ${recov}`;
}
```

In the JSX, directly above `<div className="hiking-recovery">`:

```tsx
      {(detail.recovery_summary.resting_hr != null ||
        detail.recovery_summary.hrv != null) && (
        <div className="hiking-verdict">
          {detail.recovery_summary.resting_hr != null && (
            <span>{metricVerdict("Resting HR", detail.recovery_summary.resting_hr, "bpm")}</span>
          )}
          {detail.recovery_summary.hrv != null && (
            <span>{metricVerdict("HRV", detail.recovery_summary.hrv, "ms")}</span>
          )}
        </div>
      )}
```

- [ ] **Step 2: Add the CSS rule**

In `src/styles.css`, after the `.hiking-tab .hiking-superlatives` rule (~line 3285):

```css
.hiking-tab .hiking-verdict { display: flex; flex-direction: column; gap: 4px; margin: 10px 0 4px; color: var(--text-muted); font-size: 13px; }
```

- [ ] **Step 3: Type-check and build**

```bash
npx tsc --noEmit && npm run build
```

Expected: both clean.

- [ ] **Step 4: Commit**

```bash
git add src/components/HikingTripDetail.tsx src/styles.css
git commit -m "feat(hiking): recovery verdict block on trip detail

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Full verification + visual check

**Files:** none (verification only)

- [ ] **Step 1: Run every gate**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --features web --manifest-path src-tauri/Cargo.toml hiking
GARMIN_DB_TEST=/tmp/garmin.db cargo test --features web --manifest-path src-tauri/Cargo.toml store:: -- --ignored
cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml
npx tsc --noEmit && npm run build
```

Expected: 36 passed / 2 ignored; 3 passed; check clean; frontend clean.

- [ ] **Step 2: Visual check against real data**

Run the web server locally against the real garmin.db copy (serves the built frontend on port 8080; use a throwaway data dir so production DuckDB state is untouched):

```bash
FIT_DASHBOARD_GARMIN_DB=/tmp/garmin.db FIT_DASHBOARD_DATA_DIR=/tmp/fit-dev-data \
  cargo run --features web --manifest-path src-tauri/Cargo.toml
```

Open http://localhost:8080 (unlock/login may be required — mint at most one session). Verify on:
- **A 2022 trip (e.g. any PCT segment — note: the local instance has no overrides DB, so PCT appears as 5 separate trips; that's expected here):** sleep/RHR charts show dashed baselines and the trip band; HRV baseline and HRV verdict absent (no pre-Oct-2022 HRV); training-readiness panel absent; RHR verdict line present.
- **A recent 2025/2026 multi-day trip:** all baselines present, readiness panel present, both verdict lines present, ±14-day window visible around the shaded trip band.

Then kill the server (`Ctrl-C` / `pkill -f fit-dashboard-core`).

- [ ] **Step 3: Report**

No commit. Report verification results; deployment to CT 115 (runbook in memory: `fit-hiking-dashboard-project.md`) is a separate follow-up decision for the user.
