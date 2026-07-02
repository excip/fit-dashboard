# Hiking Module — Phase 1 Implementation Plan (engine + overview tab)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In a fork of fit-dashboard, add a read-only `garmin.db` data path and a hiking analytics engine (classify → cluster → aggregate), exposed via `/api/hiking/overview` and `/api/hiking/trips`, and a React **Hiking** tab with a year switcher, four stat cards, and three category lists (day / weekend / thru).

**Architecture:** Rust/Axum backend reads `garmin.db` (SQLite) read-only via `rusqlite`; a pure `hiking` module computes classification and trip clustering in memory; new Axum handlers mirror the existing `overview` handler; a React tab in `Dashboard.tsx` reuses the app's style and `lib/api.ts` client.

**Tech Stack:** Rust (axum 0.8, duckdb 1.2, +rusqlite 0.31 bundled, chrono, serde), React 18 + TypeScript + Vite + Zustand + ECharts.

**Spec:** `docs/superpowers/specs/2026-07-02-hiking-module-design.md`

**Scope:** Phase 1 only. Phase 2 (trip drill-down + recovery) and Phase 3 (overrides UI + settings + stitched map) are separate plans. Phase 1 *does* create the overrides/settings storage the engine reads, but the editing UI is Phase 3; for now settings use built-in defaults.

**Repo:** the fit-dashboard fork (clone of `github.com/arpanghosh8453/fit-dashboard`). Paths below are relative to the repo root. Backend = `src-tauri/`, frontend = `src/`.

---

### Task 1: Fork setup + read-only garmin.db path

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/main.rs` (data-dir/env resolution area, ~lines 20-45)
- Modify: `src-tauri/src/state.rs`
- Modify: `docker/docker-compose.yml`

- [ ] **Step 1: Fork + clone + baseline build**

```bash
# fork on GitHub, then:
git clone git@github.com:<you>/fit-dashboard.git && cd fit-dashboard
git checkout -b hiking-module
cargo build --features web --manifest-path src-tauri/Cargo.toml
npm install && npm run build
```
Expected: backend compiles, frontend builds. This proves the toolchain before any changes.

- [ ] **Step 2: Add rusqlite dependency**

In `src-tauri/Cargo.toml` under `[dependencies]` add:
```toml
rusqlite = { version = "0.31", features = ["bundled"] }
```
Run: `cargo build --features web --manifest-path src-tauri/Cargo.toml`
Expected: PASS (rusqlite bundled, no system sqlite needed).

- [ ] **Step 3: Resolve the garmin.db path from env**

In `src-tauri/src/main.rs`, next to `resolve_data_dir()` (~line 20), add:
```rust
fn resolve_garmin_db() -> Option<std::path::PathBuf> {
    std::env::var("FIT_DASHBOARD_GARMIN_DB").ok().map(std::path::PathBuf::from)
}
```
Then where `StorageInfo`/`AppState` is constructed, pass the resolved path through (see Step 4).

- [ ] **Step 4: Carry the garmin.db path in AppState**

In `src-tauri/src/state.rs`, add a field to `AppState`:
```rust
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub storage: Arc<StorageInfo>,
    pub garmin_db_path: Option<Arc<std::path::PathBuf>>,
}

impl AppState {
    pub fn new(db: Database, storage: StorageInfo, garmin_db_path: Option<std::path::PathBuf>) -> Self {
        Self {
            db: Arc::new(db),
            storage: Arc::new(storage),
            garmin_db_path: garmin_db_path.map(Arc::new),
        }
    }
}
```
Update the `AppState::new(...)` call site in `main.rs` to pass `resolve_garmin_db()`.
Run: `cargo build --features web --manifest-path src-tauri/Cargo.toml`
Expected: PASS.

- [ ] **Step 5: Mount garmin.db read-only in compose**

In `docker/docker-compose.yml`, under the `fit-dashboard` service add to `environment`:
```yaml
      FIT_DASHBOARD_GARMIN_DB: /garmin/garmin.db
```
and to `volumes`:
```yaml
      - /opt/appdata/fitness/givemydata/garmin.db:/garmin/garmin.db:ro
```
(No runtime test here — verified end-to-end in Task 10.)

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/main.rs src-tauri/src/state.rs docker/docker-compose.yml
git commit -m "feat(hiking): read-only garmin.db path via rusqlite + env/compose"
```

---

### Task 2: Hiking domain types + haversine

**Files:**
- Create: `src-tauri/src/hiking/mod.rs`
- Modify: `src-tauri/src/main.rs` (add `mod hiking;`)

- [ ] **Step 1: Declare the module**

In `src-tauri/src/main.rs` with the other `mod` lines add: `mod hiking;`

- [ ] **Step 2: Write the failing test (types + haversine)**

Create `src-tauri/src/hiking/mod.rs`:
```rust
use chrono::NaiveDate;

pub mod classify;
pub mod cluster;
pub mod aggregate;

/// One candidate activity pulled from garmin.db.
#[derive(Debug, Clone)]
pub struct HikeActivity {
    pub activity_id: i64,
    pub date: NaiveDate,
    pub activity_type: String,
    pub distance_m: f64,
    pub elevation_gain: f64,
    pub elevation_loss: f64,
    pub steps: i64,
    pub start_lat: f64,
    pub start_lon: f64,
    pub end_lat: f64,
    pub end_lon: f64,
    pub max_elevation: Option<f64>,
    pub location_name: Option<String>,
    pub duration_s: f64,
}

/// Tunable settings (Phase 1: built-in defaults).
#[derive(Debug, Clone)]
pub struct HikingSettings {
    pub home_lat: f64,
    pub home_lon: f64,
    pub min_gain_m: f64,
    pub min_distance_m: f64,
    pub link_radius_m: f64,
    pub max_rest_days: i64,
}

impl Default for HikingSettings {
    fn default() -> Self {
        Self {
            home_lat: 47.40,
            home_lon: 8.05,
            min_gain_m: 250.0,
            min_distance_m: 12_000.0,
            link_radius_m: 7_500.0,
            max_rest_days: 4,
        }
    }
}

/// Per-activity manual override (Phase 3 writes these; Phase 1 just honors them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Override {
    ForceHike,
    ForceWalk,
}

/// Great-circle distance in metres.
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0_f64;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_known_distance() {
        // Aarau (47.39,8.05) to Andermatt (46.63,8.60) ~ 90 km
        let d = haversine_m(47.39, 8.05, 46.63, 8.60);
        assert!((80_000.0..100_000.0).contains(&d), "got {d}");
    }
}
```

- [ ] **Step 3: Create empty submodule files so it compiles**

Create `src-tauri/src/hiking/classify.rs`, `src-tauri/src/hiking/cluster.rs`, `src-tauri/src/hiking/aggregate.rs` each containing just `// implemented in later tasks`.

- [ ] **Step 4: Run the test**

Run: `cargo test --features web --manifest-path src-tauri/Cargo.toml haversine_known_distance`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/hiking src-tauri/src/main.rs
git commit -m "feat(hiking): domain types + haversine"
```

---

### Task 3: Classification (`is_hike`)

**Files:**
- Modify: `src-tauri/src/hiking/classify.rs`

- [ ] **Step 1: Write the failing tests**

`src-tauri/src/hiking/classify.rs`:
```rust
use crate::hiking::{HikeActivity, HikingSettings, Override};

const POOL: [&str; 3] = ["hiking", "walking", "snow_shoe"];

/// True if this activity should be treated as a hike.
pub fn is_hike(a: &HikeActivity, s: &HikingSettings, ovr: Option<Override>) -> bool {
    match ovr {
        Some(Override::ForceHike) => return true,
        Some(Override::ForceWalk) => return false,
        None => {}
    }
    if !POOL.contains(&a.activity_type.as_str()) {
        return false;
    }
    a.activity_type == "hiking"
        || a.elevation_gain > s.min_gain_m
        || a.distance_m > s.min_distance_m
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn act(t: &str, dist: f64, gain: f64) -> HikeActivity {
        HikeActivity {
            activity_id: 1,
            date: NaiveDate::from_ymd_opt(2022, 5, 3).unwrap(),
            activity_type: t.into(),
            distance_m: dist,
            elevation_gain: gain,
            elevation_loss: 0.0,
            steps: 0,
            start_lat: 0.0, start_lon: 0.0, end_lat: 0.0, end_lon: 0.0,
            max_elevation: None, location_name: None, duration_s: 0.0,
        }
    }

    #[test]
    fn pct_walking_day_is_hike_by_gain_and_distance() {
        // real: 33.2km / 1338m gain, logged as "walking"
        assert!(is_hike(&act("walking", 33_200.0, 1338.0), &HikingSettings::default(), None));
    }

    #[test]
    fn home_walk_is_not_hike() {
        // real: 4.5km / 20m gain
        assert!(!is_hike(&act("walking", 4_500.0, 20.0), &HikingSettings::default(), None));
    }

    #[test]
    fn garmin_hiking_label_always_hike() {
        assert!(is_hike(&act("hiking", 1_000.0, 50.0), &HikingSettings::default(), None));
    }

    #[test]
    fn running_excluded_even_if_long() {
        assert!(!is_hike(&act("trail_running", 30_000.0, 1000.0), &HikingSettings::default(), None));
    }

    #[test]
    fn override_wins_over_rule() {
        assert!(!is_hike(&act("hiking", 1_000.0, 500.0), &HikingSettings::default(), Some(Override::ForceWalk)));
        assert!(is_hike(&act("walking", 100.0, 5.0), &HikingSettings::default(), Some(Override::ForceHike)));
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --features web --manifest-path src-tauri/Cargo.toml classify::`
Expected: all 5 PASS (the implementation is in the same file).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/hiking/classify.rs
git commit -m "feat(hiking): hike vs walk classification"
```

---

### Task 4: Trip clustering

**Files:**
- Modify: `src-tauri/src/hiking/cluster.rs`

- [ ] **Step 1: Write the failing tests + implementation**

`src-tauri/src/hiking/cluster.rs`:
```rust
use std::collections::HashMap;
use chrono::NaiveDate;
use crate::hiking::{HikeActivity, HikingSettings, Override, haversine_m};
use crate::hiking::classify::is_hike;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TripCategory { DayHike, Weekend, ThruHike }

#[derive(Debug, Clone, serde::Serialize)]
pub struct Trip {
    pub id: i64,               // = first member activity_id (stable)
    pub category: TripCategory,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub nights: i64,
    pub activity_ids: Vec<i64>,
    pub total_distance_m: f64,
    pub total_gain: f64,
    pub total_loss: f64,
    pub total_steps: i64,
}

fn category_for(nights: i64) -> TripCategory {
    match nights {
        0 => TripCategory::DayHike,
        1 | 2 => TripCategory::Weekend,
        _ => TripCategory::ThruHike,
    }
}

/// Filter to hikes, then chain consecutive hikes into trips.
pub fn cluster_trips(
    activities: &[HikeActivity],
    settings: &HikingSettings,
    overrides: &HashMap<i64, Override>,
) -> Vec<Trip> {
    let mut hikes: Vec<&HikeActivity> = activities
        .iter()
        .filter(|a| is_hike(a, settings, overrides.get(&a.activity_id).copied()))
        .collect();
    hikes.sort_by_key(|a| (a.date, a.activity_id));

    let mut trips: Vec<Vec<&HikeActivity>> = Vec::new();
    for h in hikes {
        let joins = trips.last().map_or(false, |cur| {
            let prev = *cur.last().unwrap();
            let gap = (h.date - prev.date).num_days();
            let near = haversine_m(prev.end_lat, prev.end_lon, h.start_lat, h.start_lon)
                <= settings.link_radius_m;
            // gap of 0 (same day) up to max_rest_days rest days => <= max_rest_days + 1
            gap >= 0 && gap <= settings.max_rest_days + 1 && near
        });
        if joins {
            trips.last_mut().unwrap().push(h);
        } else {
            trips.push(vec![h]);
        }
    }

    trips.into_iter().map(|members| {
        let start_date = members.first().unwrap().date;
        let end_date = members.last().unwrap().date;
        let nights = (end_date - start_date).num_days();
        Trip {
            id: members.first().unwrap().activity_id,
            category: category_for(nights),
            start_date,
            end_date,
            nights,
            activity_ids: members.iter().map(|m| m.activity_id).collect(),
            total_distance_m: members.iter().map(|m| m.distance_m).sum(),
            total_gain: members.iter().map(|m| m.elevation_gain).sum(),
            total_loss: members.iter().map(|m| m.elevation_loss).sum(),
            total_steps: members.iter().map(|m| m.steps).sum(),
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(id: i64, y: i32, m: u32, d: u32, slat: f64, slon: f64, elat: f64, elon: f64, gain: f64) -> HikeActivity {
        HikeActivity {
            activity_id: id,
            date: NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            activity_type: "walking".into(),
            distance_m: 20_000.0, elevation_gain: gain, elevation_loss: gain,
            steps: 25_000,
            start_lat: slat, start_lon: slon, end_lat: elat, end_lon: elon,
            max_elevation: None, location_name: None, duration_s: 0.0,
        }
    }

    fn defaults() -> (HikingSettings, HashMap<i64, Override>) {
        (HikingSettings::default(), HashMap::new())
    }

    #[test]
    fn consecutive_spatially_linked_days_form_one_thru_hike() {
        // 5 days, each day's end == next day's start, all with big gain => thru-hike
        let acts = vec![
            h(1, 2022, 5, 1, 34.0,-117.0, 34.1,-117.1, 500.0),
            h(2, 2022, 5, 2, 34.1,-117.1, 34.2,-117.2, 700.0),
            h(3, 2022, 5, 3, 34.2,-117.2, 34.3,-117.3, 1338.0),
            h(4, 2022, 5, 4, 34.3,-117.3, 34.4,-117.4, 825.0),
            h(5, 2022, 5, 5, 34.4,-117.4, 34.5,-117.5, 900.0),
        ];
        let (s, o) = defaults();
        let trips = cluster_trips(&acts, &s, &o);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].category, TripCategory::ThruHike);
        assert_eq!(trips[0].nights, 4);
        assert_eq!(trips[0].activity_ids, vec![1,2,3,4,5]);
    }

    #[test]
    fn rest_day_within_tolerance_keeps_trip_together() {
        // day 1 then day 4 (2 rest days) at the same place => still one trip
        let acts = vec![
            h(1, 2022, 5, 1, 34.0,-117.0, 34.1,-117.1, 500.0),
            h(2, 2022, 5, 4, 34.1,-117.1, 34.2,-117.2, 700.0),
        ];
        let (s, o) = defaults();
        let trips = cluster_trips(&acts, &s, &o);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].nights, 3);
        assert_eq!(trips[0].category, TripCategory::ThruHike);
    }

    #[test]
    fn far_apart_days_are_separate_trips() {
        // consecutive dates but 200km apart => two trips (two day hikes)
        let acts = vec![
            h(1, 2022, 5, 1, 34.0,-117.0, 34.0,-117.0, 500.0),
            h(2, 2022, 5, 2, 47.0,   8.0, 47.0,   8.0, 500.0),
        ];
        let (s, o) = defaults();
        let trips = cluster_trips(&acts, &s, &o);
        assert_eq!(trips.len(), 2);
        assert!(trips.iter().all(|t| t.category == TripCategory::DayHike));
    }

    #[test]
    fn one_night_is_weekend() {
        let acts = vec![
            h(1, 2026, 6, 26, 46.5,8.9, 46.55,8.8, 1398.0),
            h(2, 2026, 6, 27, 46.55,8.8, 46.59,8.67, 1224.0),
        ];
        let (s, o) = defaults();
        let trips = cluster_trips(&acts, &s, &o);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].category, TripCategory::Weekend);
        assert_eq!(trips[0].nights, 1);
    }

    #[test]
    fn home_walks_are_excluded_from_trips() {
        let mut walk = h(1, 2026, 6, 1, 47.4,8.05, 47.4,8.05, 20.0);
        walk.distance_m = 4_500.0;
        let (s, o) = defaults();
        assert!(cluster_trips(&[walk], &s, &o).is_empty());
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --features web --manifest-path src-tauri/Cargo.toml cluster::`
Expected: all 5 PASS.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/hiking/cluster.rs
git commit -m "feat(hiking): trip clustering (day/weekend/thru) with rest-day tolerance"
```

---

### Task 5: Aggregation + superlatives

**Files:**
- Modify: `src-tauri/src/hiking/aggregate.rs`

- [ ] **Step 1: Write the failing tests + implementation**

`src-tauri/src/hiking/aggregate.rs`:
```rust
use crate::hiking::HikeActivity;
use crate::hiking::cluster::Trip;

#[derive(Debug, Clone, serde::Serialize)]
pub struct OverviewStats {
    pub year: Option<i32>,          // None = all-time
    pub total_distance_m: f64,
    pub total_steps: i64,
    pub total_gain: f64,
    pub total_loss: f64,
    pub hike_count: usize,
    pub trip_count: usize,
}

/// Roll up trips into headline stats. `year=None` => all-time; else trips whose
/// start_date is in that year.
pub fn overview(trips: &[Trip], year: Option<i32>) -> OverviewStats {
    use chrono::Datelike;
    let sel: Vec<&Trip> = trips.iter()
        .filter(|t| year.map_or(true, |y| t.start_date.year() == y))
        .collect();
    OverviewStats {
        year,
        total_distance_m: sel.iter().map(|t| t.total_distance_m).sum(),
        total_steps: sel.iter().map(|t| t.total_steps).sum(),
        total_gain: sel.iter().map(|t| t.total_gain).sum(),
        total_loss: sel.iter().map(|t| t.total_loss).sum(),
        hike_count: sel.iter().map(|t| t.activity_ids.len()).sum(),
        trip_count: sel.len(),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Superlatives {
    pub longest_day_m: f64,
    pub biggest_climb_m: f64,
    pub highest_point_m: Option<f64>,
}

/// Superlatives for a single trip, from its member activities.
pub fn superlatives(trip: &Trip, activities: &[HikeActivity]) -> Superlatives {
    let members: Vec<&HikeActivity> = activities.iter()
        .filter(|a| trip.activity_ids.contains(&a.activity_id))
        .collect();
    Superlatives {
        longest_day_m: members.iter().map(|m| m.distance_m).fold(0.0, f64::max),
        biggest_climb_m: members.iter().map(|m| m.elevation_gain).fold(0.0, f64::max),
        highest_point_m: members.iter().filter_map(|m| m.max_elevation).fold(None, |acc, v| {
            Some(acc.map_or(v, |a: f64| a.max(v)))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hiking::cluster::{Trip, TripCategory};
    use chrono::NaiveDate;

    fn trip(id: i64, y: i32, dist: f64, ids: Vec<i64>) -> Trip {
        Trip {
            id, category: TripCategory::ThruHike,
            start_date: NaiveDate::from_ymd_opt(y,5,1).unwrap(),
            end_date: NaiveDate::from_ymd_opt(y,5,5).unwrap(),
            nights: 4, activity_ids: ids,
            total_distance_m: dist, total_gain: 100.0, total_loss: 100.0, total_steps: 1000,
        }
    }

    #[test]
    fn overview_filters_by_year() {
        let trips = vec![trip(1,2022,800_000.0,vec![1,2]), trip(2,2024,100_000.0,vec![3])];
        let all = overview(&trips, None);
        assert_eq!(all.trip_count, 2);
        assert_eq!(all.hike_count, 3);
        assert_eq!(overview(&trips, Some(2022)).total_distance_m, 800_000.0);
        assert_eq!(overview(&trips, Some(2024)).trip_count, 1);
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --features web --manifest-path src-tauri/Cargo.toml aggregate::`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/hiking/aggregate.rs
git commit -m "feat(hiking): overview aggregation + trip superlatives"
```

---

### Task 6: garmin.db read layer (store)

**Files:**
- Create: `src-tauri/src/hiking/store.rs`
- Modify: `src-tauri/src/hiking/mod.rs` (add `pub mod store;`)

- [ ] **Step 1: Implement the read layer**

`src-tauri/src/hiking/store.rs`:
```rust
use std::path::Path;
use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::Connection;
use crate::hiking::HikeActivity;

/// Read all foot/trail candidate activities from garmin.db (read-only).
pub fn load_activities(garmin_db: &Path) -> Result<Vec<HikeActivity>> {
    let conn = Connection::open_with_flags(
        garmin_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut stmt = conn.prepare(
        "SELECT activity_id, substr(start_time_local,1,10) AS d, activity_type,
                COALESCE(distance_meters,0), COALESCE(elevation_gain,0), COALESCE(elevation_loss,0),
                COALESCE(activity_steps,0), COALESCE(start_latitude,0), COALESCE(start_longitude,0),
                COALESCE(end_latitude,0), COALESCE(end_longitude,0), max_elevation,
                location_name, COALESCE(duration_seconds,0)
         FROM activity
         WHERE activity_type IN ('hiking','walking','snow_shoe')
           AND start_latitude IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        let ds: String = r.get(1)?;
        let date = NaiveDate::parse_from_str(&ds, "%Y-%m-%d")
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
        Ok(HikeActivity {
            activity_id: r.get(0)?, date, activity_type: r.get(2)?,
            distance_m: r.get(3)?, elevation_gain: r.get(4)?, elevation_loss: r.get(5)?,
            steps: r.get(6)?, start_lat: r.get(7)?, start_lon: r.get(8)?,
            end_lat: r.get(9)?, end_lon: r.get(10)?, max_elevation: r.get(11)?,
            location_name: r.get(12)?, duration_s: r.get(13)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}
```
Add `pub mod store;` to `src-tauri/src/hiking/mod.rs`.

- [ ] **Step 2: Integration test against a real garmin.db copy**

Copy a real `garmin.db` to a test fixture path on the dev machine (e.g. `/tmp/garmin.db`, scp'd from CT 115). Add:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore] // run with: cargo test -- --ignored (needs a real garmin.db at GARMIN_DB_TEST)
    fn loads_activities_from_real_db() {
        let p = std::env::var("GARMIN_DB_TEST").expect("set GARMIN_DB_TEST");
        let acts = load_activities(std::path::Path::new(&p)).unwrap();
        assert!(acts.len() > 100, "expected many activities, got {}", acts.len());
        assert!(acts.iter().any(|a| a.activity_type == "walking"));
    }
}
```

- [ ] **Step 3: Run it**

Run: `GARMIN_DB_TEST=/tmp/garmin.db cargo test --features web --manifest-path src-tauri/Cargo.toml store:: -- --ignored`
Expected: PASS (loads 100s of activities).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/hiking/store.rs src-tauri/src/hiking/mod.rs
git commit -m "feat(hiking): read-only garmin.db activity loader"
```

---

### Task 7: API endpoints (`/api/hiking/overview`, `/api/hiking/trips`)

**Files:**
- Create: `src-tauri/src/hiking/http.rs`
- Modify: `src-tauri/src/hiking/mod.rs` (`pub mod http;`)
- Modify: `src-tauri/src/server.rs` (register routes in `app()` ~line 54; import auth guard used by other handlers)

- [ ] **Step 1: Write the handlers (mirror the existing `overview` handler pattern)**

`src-tauri/src/hiking/http.rs`:
```rust
use std::collections::HashMap;
use std::sync::Arc;
use axum::{extract::{Query, State}, http::StatusCode, Json};
use crate::state::AppState;
use crate::hiking::{HikingSettings, store, cluster, aggregate};

#[derive(serde::Deserialize)]
pub struct YearQuery { pub year: Option<i32> }

fn load_trips(state: &AppState) -> Result<Vec<cluster::Trip>, StatusCode> {
    let path = state.garmin_db_path.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let acts = store::load_activities(path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let settings = HikingSettings::default();          // Phase 3: load from DuckDB
    let overrides = HashMap::new();                     // Phase 3: load from DuckDB
    Ok(cluster::cluster_trips(&acts, &settings, &overrides))
}

pub async fn hiking_overview(
    State(state): State<Arc<AppState>>,
    Query(q): Query<YearQuery>,
) -> Result<Json<aggregate::OverviewStats>, StatusCode> {
    let trips = load_trips(&state)?;
    Ok(Json(aggregate::overview(&trips, q.year)))
}

#[derive(serde::Deserialize)]
pub struct CategoryQuery { pub category: Option<String>, pub year: Option<i32> }

pub async fn hiking_trips(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CategoryQuery>,
) -> Result<Json<Vec<cluster::Trip>>, StatusCode> {
    use chrono::Datelike;
    let mut trips = load_trips(&state)?;
    if let Some(y) = q.year { trips.retain(|t| t.start_date.year() == y); }
    if let Some(cat) = q.category.as_deref() {
        let want = match cat {
            "day_hike" => cluster::TripCategory::DayHike,
            "weekend" => cluster::TripCategory::Weekend,
            "thru_hike" => cluster::TripCategory::ThruHike,
            _ => return Err(StatusCode::BAD_REQUEST),
        };
        trips.retain(|t| t.category == want);
    }
    trips.sort_by(|a, b| b.start_date.cmp(&a.start_date));
    Ok(Json(trips))
}
```
Add `pub mod http;` to `src-tauri/src/hiking/mod.rs`.

- [ ] **Step 2: Register routes**

In `src-tauri/src/server.rs`, in `app()` (after the `/api/overview` route ~line 54) add:
```rust
        .route("/api/hiking/overview", get(crate::hiking::http::hiking_overview))
        .route("/api/hiking/trips", get(crate::hiking::http::hiking_trips))
```
These sit behind the same session middleware as the other `/api/*` routes (verify against how `/api/overview` is guarded in `app()`; apply the identical layer/guard).

- [ ] **Step 3: Build**

Run: `cargo build --features web --manifest-path src-tauri/Cargo.toml`
Expected: PASS.

- [ ] **Step 4: Manual endpoint check (with a real garmin.db)**

Run the server locally pointing at a real db:
```bash
FIT_DASHBOARD_GARMIN_DB=/tmp/garmin.db cargo run --features web --manifest-path src-tauri/Cargo.toml &
# onboard/unlock to get a token T, then:
curl -s "http://localhost:8080/api/hiking/overview" -H "X-Session: $T" | jq
curl -s "http://localhost:8080/api/hiking/trips?category=thru_hike" -H "X-Session: $T" | jq 'length'
```
Expected: overview returns non-zero totals; thru_hike list is non-empty and includes a 2022 trip.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/hiking/http.rs src-tauri/src/hiking/mod.rs src-tauri/src/server.rs
git commit -m "feat(hiking): /api/hiking/overview and /api/hiking/trips endpoints"
```

---

### Task 8: Frontend API client

**Files:**
- Modify: `src/lib/api.ts`
- Modify: `src/types.ts`

- [ ] **Step 1: Add types**

In `src/types.ts` add:
```ts
export type TripCategory = "day_hike" | "weekend" | "thru_hike";

export interface HikingOverview {
  year: number | null;
  total_distance_m: number;
  total_steps: number;
  total_gain: number;
  total_loss: number;
  hike_count: number;
  trip_count: number;
}

export interface Trip {
  id: number;
  category: TripCategory;
  start_date: string;
  end_date: string;
  nights: number;
  activity_ids: number[];
  total_distance_m: number;
  total_gain: number;
  total_loss: number;
  total_steps: number;
}
```

- [ ] **Step 2: Add client functions (mirror existing `overview()` in api.ts)**

In `src/lib/api.ts`, alongside the other functions on the exported api object, add:
```ts
  async hikingOverview(year?: number): Promise<HikingOverview> {
    return (await ha.get("/hiking/overview", { params: year != null ? { year } : {} })).data;
  },
  async hikingTrips(category?: TripCategory, year?: number): Promise<Trip[]> {
    return (await ha.get("/hiking/trips", { params: { category, year } })).data;
  },
```
(Import `HikingOverview`, `Trip`, `TripCategory` from `../types`. `ha` is the existing axios instance with `baseURL` `/api` and the `X-Session` interceptor.)

- [ ] **Step 3: Typecheck**

Run: `npx tsc --noEmit`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/lib/api.ts src/types.ts
git commit -m "feat(hiking): frontend api client + types"
```

---

### Task 9: Hiking tab UI

**Files:**
- Create: `src/components/HikingTab.tsx`
- Modify: `src/components/Dashboard.tsx` (add the tab to the existing view switcher)

- [ ] **Step 1: Build the tab component**

Create `src/components/HikingTab.tsx`:
```tsx
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { HikingOverview, Trip, TripCategory } from "../types";

const KM = (m: number) => (m / 1000).toFixed(0);

export function HikingTab() {
  const [year, setYear] = useState<number | null>(null);
  const [ov, setOv] = useState<HikingOverview | null>(null);
  const [trips, setTrips] = useState<Trip[]>([]);

  useEffect(() => {
    void api.hikingOverview(year ?? undefined).then(setOv);
    void api.hikingTrips(undefined, year ?? undefined).then(setTrips);
  }, [year]);

  const years = Array.from({ length: 2026 - 2017 + 1 }, (_, i) => 2017 + i);
  const byCat = (c: TripCategory) => trips.filter((t) => t.category === c);

  return (
    <div className="hiking-tab">
      <div className="hiking-years">
        <button className={year === null ? "active" : ""} onClick={() => setYear(null)}>All</button>
        {years.map((y) => (
          <button key={y} className={year === y ? "active" : ""} onClick={() => setYear(y)}>{y}</button>
        ))}
      </div>

      {ov && (
        <div className="hiking-stats">
          <div className="stat-card"><b>{KM(ov.total_distance_m)} km</b><span>distance</span></div>
          <div className="stat-card"><b>{ov.total_steps.toLocaleString()}</b><span>steps</span></div>
          <div className="stat-card"><b>+{Math.round(ov.total_gain).toLocaleString()} m</b><span>elev gain</span></div>
          <div className="stat-card"><b>-{Math.round(ov.total_loss).toLocaleString()} m</b><span>elev loss</span></div>
          <div className="stat-meta">{ov.hike_count} hikes · {ov.trip_count} trips</div>
        </div>
      )}

      {(["thru_hike", "weekend", "day_hike"] as TripCategory[]).map((cat) => (
        <section key={cat} className="hiking-section">
          <h3>{cat === "thru_hike" ? "Thru-hikes (3+ nights)" : cat === "weekend" ? "Weekend trips (1–2 nights)" : "Day hikes"}</h3>
          {byCat(cat).map((t) => (
            <div key={t.id} className="trip-row">
              <span className="trip-dates">{t.start_date}{t.nights > 0 ? `→${t.end_date}` : ""}</span>
              <span className="trip-nights">{t.nights > 0 ? `${t.nights}n` : "—"}</span>
              <span className="trip-km">{KM(t.total_distance_m)} km</span>
              <span className="trip-gain">▲{Math.round(t.total_gain)}</span>
            </div>
          ))}
          {byCat(cat).length === 0 && <p className="empty">None</p>}
        </section>
      ))}
    </div>
  );
}
```

- [ ] **Step 2: Register the tab in Dashboard.tsx**

In `src/components/Dashboard.tsx`, find the existing tab/view state (the app switches between views like Overview / Activities / Map). Add a `"hiking"` view: a nav button labelled "Hiking" and render `<HikingTab />` when selected. Import `{ HikingTab } from "./HikingTab"`. Mirror exactly how the sibling tabs are declared and rendered (do not invent a new nav system).

- [ ] **Step 3: Minimal styles**

In `src/styles.css` append basic rules reusing existing design tokens (colors/spacing variables already defined in that file) for `.hiking-tab`, `.hiking-years button`, `.hiking-stats`, `.stat-card`, `.hiking-section`, `.trip-row`. Match the look of existing cards/sections — inspect an existing card class first and reuse its variables.

- [ ] **Step 4: Build + eyeball**

Run: `npm run build` then run the app (`docker compose up` in `docker/`, or `npm run dev` + backend) with `FIT_DASHBOARD_GARMIN_DB` set.
Expected: a "Hiking" tab appears; selecting it shows stat cards and the three category lists; year buttons refilter.

- [ ] **Step 5: Commit**

```bash
git add src/components/HikingTab.tsx src/components/Dashboard.tsx src/styles.css
git commit -m "feat(hiking): Hiking tab (year switcher, stat cards, category lists)"
```

---

### Task 10: End-to-end verification on CT 115

**Files:** none (deployment verification)

- [ ] **Step 1: Build and deploy the fork image**

Build the fork's Docker image (per its `docker/` build), push/load it onto CT 115, and in `/opt/appdata/fitness/docker-compose.yml` swap the `image:` to the fork build and add the env + read-only volume from Task 1 Step 5. `docker compose up -d`.

- [ ] **Step 2: Verify the endpoints live**

Run:
```bash
pct exec 115 -- bash -lc '. /opt/appdata/fitness/bridge/dashboard.env; T=$(printf "{\"password\":\"%s\"}" "$DASH_PASSWORD" | curl -sS -X POST "$DASH_URL/api/unlock" -H "Content-Type: application/json" --data @- | jq -r .token); curl -sS "$DASH_URL/api/hiking/overview" -H "X-Session: $T" | jq; curl -sS "$DASH_URL/api/hiking/trips?category=thru_hike" -H "X-Session: $T" | jq "length"'
```
Expected: non-zero overview totals; thru_hike count ≥ 1.

- [ ] **Step 2: Verify the tab in the browser**

Open `https://fit.excips.org` over Tailscale → the **Hiking** tab shows stats + category lists, and the 2022 PCT appears under Thru-hikes. Spot-check that home Aarau walks are absent.

- [ ] **Step 3: Sanity-check the classification against reality**

Confirm the 2022 PCT is a single thru-hike (not fragmented), Andermatt 2026-06-28 is a day hike, and Blenio 2026-06-26→27 is a weekend trip. If clustering looks off, tune `HikingSettings` defaults (link radius / rest days) — thresholds are the knobs, not the algorithm.

---

## Notes for the executor

- **Engine is pure and fully tested** (Tasks 2–5) — run `cargo test --features web --manifest-path src-tauri/Cargo.toml hiking` to run the whole engine suite.
- **Framework wiring (Tasks 1, 7, 9) references real upstream files** — mirror the existing `overview` handler (server.rs:317) and the sibling tab declarations in `Dashboard.tsx`; do not introduce new patterns.
- **Settings/overrides are defaults-only in Phase 1**; Phase 3 adds the DuckDB tables + editing UI. Keep `load_trips()`'s two `HashMap::new()` / `::default()` seams so Phase 3 can swap them for DuckDB reads.
- **Deploy is image-swap** on CT 115 — the fork replaces `ghcr.io/arpanghosh8453/fit-dashboard:latest` in the compose `image:`.
