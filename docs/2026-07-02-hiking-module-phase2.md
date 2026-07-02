# Hiking Module — Phase 2 Implementation Plan (trip drill-down, recovery, names, merge)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clicking a trip in the Hiking tab opens a detail view with a stitched whole-trip map, per-day load list (linking to the single-activity view), superlatives, and four daily recovery trends; trips get auto-derived area names with manual rename; a "merge into previous trip" override lets the user join trips split by unrecorded GPS gaps (the 2022 PCT case).

**Architecture:** The pure engine gains a `LinkPrevious` override (joins a hike to the preceding trip unconditionally) and auto-derived trip names from `location_name`. Overrides + trip names persist in fit-dashboard's own DuckDB (new tables, so garmin.db re-syncs never clobber them). New endpoints: `GET /api/hiking/trip/{id}`, `POST .../merge-previous`, `POST .../split`, `PUT /api/hiking/trip-name`. Recovery data joins six garmin.db daily tables by `calendar_date`. The stitched map reuses the existing `/api/records/{id}` per activity (garmin `activity_id` ↔ DuckDB `activities.file_name = '<id>_ACTIVITY.fit'`).

**Tech Stack:** Rust (axum 0.8, rusqlite 0.31, duckdb, chrono, serde), React 18 + TS + maplibre-gl + echarts-for-react.

**Spec:** `docs/2026-07-02-hiking-module-design.md` (Phase 2 + the merge/name/map slices of Phase 3, pulled forward per user request). Phase 1 is complete and deployed.

**User-confirmed acceptance criterion:** the 2022 PCT (5 trips: 04-10→06-04, 06-07→06-12, 06-16→08-06, 08-07→08-12, 08-14→09-12 — split by 21–56 km unrecorded GPS gaps) must be mergeable into ONE thru-hike via the UI merge button.

**Environment notes (from Phase 1):** cargo at `~/.cargo/bin`; real garmin.db at `/tmp/garmin.db`; run engine tests with `cargo test --features web --manifest-path src-tauri/Cargo.toml hiking`; `hiking/http.rs` and any module touching `crate::server` MUST be cfg-gated `#[cfg(all(feature = "web", not(feature = "tauri-app")))]`; after backend tasks also run `cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml`. Commit messages end with a blank line then `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: Engine — `LinkPrevious` override + derived trip names (TDD)

**Files:**
- Modify: `src-tauri/src/hiking/mod.rs` (Override enum)
- Modify: `src-tauri/src/hiking/classify.rs` (match arm + 1 test)
- Modify: `src-tauri/src/hiking/cluster.rs` (join condition, `Trip.name`, derivation + 3 tests)
- Modify: `src-tauri/src/hiking/aggregate.rs` (test helper gets `name: None`)

- [ ] **Step 1: Extend the Override enum**

In `src-tauri/src/hiking/mod.rs` replace the `Override` enum with:

```rust
/// Per-activity manual override. ForceHike/ForceWalk flip classification;
/// LinkPrevious joins the activity's trip to the preceding trip regardless of
/// spatial/temporal continuity (bridges unrecorded GPS gaps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Override {
    ForceHike,
    ForceWalk,
    LinkPrevious,
}
```

- [ ] **Step 2: Make classification ignore LinkPrevious**

In `src-tauri/src/hiking/classify.rs`, change the match in `is_hike` from `None => {}` to a catch-all so `LinkPrevious` falls through to the normal rules:

```rust
    match ovr {
        Some(Override::ForceHike) => return true,
        Some(Override::ForceWalk) => return false,
        _ => {}
    }
```

Add this test inside the existing `mod tests`:

```rust
    #[test]
    fn link_override_does_not_affect_classification() {
        // a home walk with a link override is still not a hike
        assert!(!is_hike(&act("walking", 4_500.0, 20.0), &HikingSettings::default(), Some(Override::LinkPrevious)));
    }
```

- [ ] **Step 3: Trip name field + derivation + link join in cluster.rs**

In `src-tauri/src/hiking/cluster.rs`:

3a. Add the field to `Trip` (after `category`):

```rust
    pub name: Option<String>,
```

3b. Add above `cluster_trips`:

```rust
/// Auto-label from member location names: unique names in order;
/// one name => that name, several => "first → last", none => None.
fn derive_name(members: &[&HikeActivity]) -> Option<String> {
    let mut names: Vec<&str> = Vec::new();
    for m in members {
        if let Some(n) = m.location_name.as_deref() {
            if !n.is_empty() && names.last() != Some(&n) {
                names.push(n);
            }
        }
    }
    names.dedup();
    match names.len() {
        0 => None,
        1 => Some(names[0].to_string()),
        _ => Some(format!("{} → {}", names[0], names[names.len() - 1])),
    }
}
```

3c. In the `for h in hikes` loop, honor the link override (replace the existing `let joins = ...` statement):

```rust
        let linked = matches!(overrides.get(&h.activity_id), Some(Override::LinkPrevious));
        let joins = (linked && !trips.is_empty())
            || trips.last().map_or(false, |cur| {
                let prev = *cur.last().unwrap();
                let gap = (h.date - prev.date).num_days();
                let near = haversine_m(prev.end_lat, prev.end_lon, h.start_lat, h.start_lon)
                    <= settings.link_radius_m;
                // gap of 0 (same day) up to max_rest_days rest days => <= max_rest_days + 1
                gap >= 0 && gap <= settings.max_rest_days + 1 && near
            });
```

3d. In the final `Trip { ... }` construction add:

```rust
            name: derive_name(&members),
```

3e. Add these tests inside the existing `mod tests` (the `h(...)` helper exists; it sets `location_name: None`):

```rust
    #[test]
    fn link_previous_override_bridges_spatial_gap() {
        // consecutive days 200+ km apart normally split; LinkPrevious on the
        // second activity joins them into one trip
        let acts = vec![
            h(1, 2022, 5, 1, 34.0,-117.0, 34.0,-117.0, 500.0),
            h(2, 2022, 5, 2, 47.0,   8.0, 47.0,   8.0, 500.0),
        ];
        let (s, _) = defaults();
        let mut o = HashMap::new();
        o.insert(2, Override::LinkPrevious);
        let trips = cluster_trips(&acts, &s, &o);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].activity_ids, vec![1, 2]);
        assert_eq!(trips[0].nights, 1);
    }

    #[test]
    fn link_previous_on_first_hike_is_noop() {
        let acts = vec![h(1, 2022, 5, 1, 34.0,-117.0, 34.0,-117.0, 500.0)];
        let (s, _) = defaults();
        let mut o = HashMap::new();
        o.insert(1, Override::LinkPrevious);
        assert_eq!(cluster_trips(&acts, &s, &o).len(), 1);
    }

    #[test]
    fn trip_name_derived_from_location_names() {
        let mut a = h(1, 2022, 5, 1, 34.0,-117.0, 34.1,-117.1, 500.0);
        a.location_name = Some("Inyo County".into());
        let mut b = h(2, 2022, 5, 2, 34.1,-117.1, 34.2,-117.2, 500.0);
        b.location_name = Some("Inyo County".into());
        let mut c = h(3, 2022, 5, 3, 34.2,-117.2, 34.3,-117.3, 500.0);
        c.location_name = Some("Fresno County".into());
        let (s, o) = defaults();
        let trips = cluster_trips(&[a.clone(), b.clone(), c], &s, &o);
        assert_eq!(trips[0].name.as_deref(), Some("Inyo County → Fresno County"));
        let trips2 = cluster_trips(&[a, b], &s, &o);
        assert_eq!(trips2[0].name.as_deref(), Some("Inyo County"));
    }
```

- [ ] **Step 4: Fix the aggregate test helper**

In `src-tauri/src/hiking/aggregate.rs`, the `trip(...)` test helper constructs a `Trip` — add `name: None,` after `category`.

- [ ] **Step 5: Run the engine suite**

Run: `cargo test --features web --manifest-path src-tauri/Cargo.toml hiking`
Expected: 23 passed (19 prior + 1 classify + 3 cluster), 1 ignored, 0 failed. Also run `cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml` — PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/hiking
git commit -m "feat(hiking): LinkPrevious merge override + derived trip names"
```

---

### Task 2: DuckDB storage for overrides + trip names

**Files:**
- Modify: `src-tauri/src/database.rs`

- [ ] **Step 1: Schema**

In `Database::init_schema` (the `CREATE TABLE IF NOT EXISTS` block, after the `settings` table), add:

```sql
            CREATE TABLE IF NOT EXISTS hiking_overrides (
                activity_id BIGINT PRIMARY KEY,
                kind VARCHAR NOT NULL
            );
            CREATE TABLE IF NOT EXISTS hiking_trip_names (
                trip_id BIGINT PRIMARY KEY,
                name VARCHAR NOT NULL
            );
```

(Mirror the exact formatting/execution style of the surrounding schema statements.)

- [ ] **Step 2: Methods**

Add to `impl Database`, mirroring the style of `get_setting`/`set_setting` (same lock/prepare/query patterns the file already uses):

```rust
    pub fn hiking_overrides(&self) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT activity_id, kind FROM hiking_overrides")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn set_hiking_override(&self, activity_id: i64, kind: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        match kind {
            Some(k) => {
                conn.execute(
                    "INSERT OR REPLACE INTO hiking_overrides (activity_id, kind) VALUES (?, ?)",
                    duckdb::params![activity_id, k],
                )?;
            }
            None => {
                conn.execute("DELETE FROM hiking_overrides WHERE activity_id = ?", duckdb::params![activity_id])?;
            }
        }
        Ok(())
    }

    pub fn hiking_trip_names(&self) -> Result<Vec<(i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT trip_id, name FROM hiking_trip_names")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn set_hiking_trip_name(&self, trip_id: i64, name: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        match name {
            Some(n) => {
                conn.execute(
                    "INSERT OR REPLACE INTO hiking_trip_names (trip_id, name) VALUES (?, ?)",
                    duckdb::params![trip_id, n],
                )?;
            }
            None => {
                conn.execute("DELETE FROM hiking_trip_names WHERE trip_id = ?", duckdb::params![trip_id])?;
            }
        }
        Ok(())
    }

    pub fn activity_id_by_file_name(&self, file_name: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM activities WHERE file_name = ? LIMIT 1")?;
        let mut rows = stmt.query(duckdb::params![file_name])?;
        Ok(match rows.next()? { Some(row) => Some(row.get(0)?), None => None })
    }
```

IMPORTANT: read the actual file first — the lock/param/exec idioms above are a sketch; mirror EXACTLY how existing methods access the connection (`self.conn.lock()`, params macro name, error style). Behavior (upsert-or-delete semantics, exact SQL) must stay as specified. If DuckDB rejects `INSERT OR REPLACE`, use `DELETE` followed by `INSERT` inside the same lock, and note it in your report.

- [ ] **Step 3: Test**

Add (or extend) a `#[cfg(test)] mod tests` in `database.rs`:

```rust
    #[test]
    fn hiking_override_and_name_roundtrip() {
        let dir = std::env::temp_dir().join(format!("fitdash-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::new(dir.join("t.duckdb").to_str().unwrap()).unwrap();
        db.set_hiking_override(42, Some("link_previous")).unwrap();
        db.set_hiking_override(43, Some("force_walk")).unwrap();
        db.set_hiking_override(43, None).unwrap();
        assert_eq!(db.hiking_overrides().unwrap(), vec![(42, "link_previous".to_string())]);
        db.set_hiking_trip_name(42, Some("PCT 2022")).unwrap();
        assert_eq!(db.hiking_trip_names().unwrap(), vec![(42, "PCT 2022".to_string())]);
        db.set_hiking_trip_name(42, None).unwrap();
        assert!(db.hiking_trip_names().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
```

(Adapt `Database::new`'s argument if its real signature differs.)

- [ ] **Step 4: Run**

Run: `cargo test --features web --manifest-path src-tauri/Cargo.toml hiking_override_and_name_roundtrip`
Expected: PASS. Also `cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml` — PASS (database.rs is shared by both builds; the new methods will be dead code in tauri builds — if that produces warnings, that is acceptable; do NOT cfg-gate Database methods).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/database.rs
git commit -m "feat(hiking): DuckDB storage for overrides and trip names"
```

---

### Task 3: Recovery loader from garmin.db (integration-tested)

**Files:**
- Modify: `src-tauri/src/hiking/store.rs`

- [ ] **Step 1: Implement `load_recovery`**

Append to `src-tauri/src/hiking/store.rs`:

```rust
/// One calendar day of recovery metrics joined from garmin.db daily tables.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoveryDay {
    pub date: String,
    pub sleep_score: Option<i64>,
    pub sleep_seconds: Option<i64>,
    pub resting_hr: Option<i64>,
    pub hrv_last_night_avg: Option<f64>,
    pub body_battery_high: Option<i64>,
    pub body_battery_low: Option<i64>,
    pub avg_stress: Option<i64>,
    pub training_readiness: Option<f64>,
}

/// Load daily recovery metrics for [start, end] inclusive (read-only).
pub fn load_recovery(garmin_db: &Path, start: NaiveDate, end: NaiveDate) -> Result<Vec<RecoveryDay>> {
    use std::collections::HashMap;
    let conn = Connection::open_with_flags(
        garmin_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let (s, e) = (start.to_string(), end.to_string());

    fn fetch<T: rusqlite::types::FromSql>(
        conn: &Connection, sql: &str, s: &str, e: &str,
    ) -> Result<HashMap<String, T>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(rusqlite::params![s, e], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, T>(1)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    let range = " WHERE calendar_date BETWEEN ?1 AND ?2";
    let sleep_score: HashMap<String, i64> =
        fetch(&conn, &format!("SELECT calendar_date, sleep_score_overall FROM sleep{range}"), &s, &e)?;
    let sleep_secs: HashMap<String, i64> =
        fetch(&conn, &format!("SELECT calendar_date, sleep_time_seconds FROM sleep{range}"), &s, &e)?;
    let rhr: HashMap<String, i64> =
        fetch(&conn, &format!("SELECT calendar_date, resting_hr FROM heart_rate{range}"), &s, &e)?;
    let hrv: HashMap<String, f64> =
        fetch(&conn, &format!("SELECT calendar_date, last_night_avg FROM hrv{range}"), &s, &e)?;
    let bb_hi: HashMap<String, i64> =
        fetch(&conn, &format!("SELECT calendar_date, highest FROM body_battery{range}"), &s, &e)?;
    let bb_lo: HashMap<String, i64> =
        fetch(&conn, &format!("SELECT calendar_date, lowest FROM body_battery{range}"), &s, &e)?;
    let stress: HashMap<String, i64> =
        fetch(&conn, &format!("SELECT calendar_date, avg_stress FROM stress{range}"), &s, &e)?;
    let readiness: HashMap<String, f64> =
        fetch(&conn, &format!("SELECT calendar_date, score FROM training_readiness{range}"), &s, &e)?;

    let mut out = Vec::new();
    let mut d = start;
    while d <= end {
        let key = d.to_string();
        out.push(RecoveryDay {
            date: key.clone(),
            sleep_score: sleep_score.get(&key).copied(),
            sleep_seconds: sleep_secs.get(&key).copied(),
            resting_hr: rhr.get(&key).copied(),
            hrv_last_night_avg: hrv.get(&key).copied(),
            body_battery_high: bb_hi.get(&key).copied(),
            body_battery_low: bb_lo.get(&key).copied(),
            avg_stress: stress.get(&key).copied(),
            training_readiness: readiness.get(&key).copied(),
        });
        d = d.succ_opt().unwrap();
    }
    Ok(out)
}
```

Note: NULL cells — `fetch` uses `filter_map(ok)`, so a row whose value column is NULL is simply skipped (treated as absent). That is the intended semantic.

- [ ] **Step 2: Integration test**

Add to the existing `mod tests` in store.rs:

```rust
    #[test]
    #[ignore] // needs a real garmin.db at GARMIN_DB_TEST
    fn loads_recovery_from_real_db() {
        let p = std::env::var("GARMIN_DB_TEST").expect("set GARMIN_DB_TEST");
        let days = load_recovery(
            std::path::Path::new(&p),
            chrono::NaiveDate::from_ymd_opt(2022, 5, 1).unwrap(),
            chrono::NaiveDate::from_ymd_opt(2022, 5, 5).unwrap(),
        ).unwrap();
        assert_eq!(days.len(), 5);
        assert!(days.iter().any(|d| d.resting_hr.is_some()), "expected some resting HR data");
    }
```

- [ ] **Step 3: Run**

Run: `GARMIN_DB_TEST=/tmp/garmin.db cargo test --features web --manifest-path src-tauri/Cargo.toml store:: -- --ignored`
Expected: 2 passed (the Phase 1 loader test + this one).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/hiking/store.rs
git commit -m "feat(hiking): daily recovery loader from garmin.db health tables"
```

---

### Task 4: HTTP — trip detail, merge/split, rename; overrides wired into clustering

**Files:**
- Modify: `src-tauri/src/hiking/http.rs`
- Modify: `src-tauri/src/server.rs` (4 new routes)

- [ ] **Step 1: Wire persisted overrides + names into the existing handlers**

In `src-tauri/src/hiking/http.rs`:

1a. Replace `load_trips` so it reads overrides from DuckDB and overlays name overrides (keep the existing tracing error style; `ensure_session` stays in the handlers):

```rust
fn parse_override(kind: &str) -> Option<Override> {
    match kind {
        "force_hike" => Some(Override::ForceHike),
        "force_walk" => Some(Override::ForceWalk),
        "link_previous" => Some(Override::LinkPrevious),
        _ => None,
    }
}

fn load_overrides(state: &AppState) -> HashMap<i64, Override> {
    state.db.hiking_overrides().unwrap_or_default().into_iter()
        .filter_map(|(id, k)| parse_override(&k).map(|o| (id, o)))
        .collect()
}

fn load_acts_and_trips(state: &AppState) -> Result<(Vec<crate::hiking::HikeActivity>, Vec<cluster::Trip>), StatusCode> {
    let path = state.garmin_db_path.as_ref().ok_or_else(|| {
        tracing::warn!("hiking endpoint unavailable: garmin db not configured");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    // re-read per request is ~10ms on real data; caching deferred
    let acts = store::load_activities(path).map_err(|e| {
        tracing::error!(error = %e, "garmin.db load failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let settings = HikingSettings::default();          // Phase 3: load from DuckDB
    let overrides = load_overrides(state);
    let mut trips = cluster::cluster_trips(&acts, &settings, &overrides);
    for (id, name) in state.db.hiking_trip_names().unwrap_or_default() {
        if let Some(t) = trips.iter_mut().find(|t| t.id == id) {
            t.name = Some(name);
        }
    }
    Ok((acts, trips))
}

fn load_trips(state: &AppState) -> Result<Vec<cluster::Trip>, StatusCode> {
    Ok(load_acts_and_trips(state)?.1)
}
```

(Keep `Override` imported from `crate::hiking`. The existing `hiking_overview`/`hiking_trips` handlers keep calling `load_trips` unchanged.)

- [ ] **Step 2: Trip detail endpoint**

Add to http.rs:

```rust
#[derive(serde::Serialize)]
pub struct TripDay {
    pub garmin_activity_id: i64,
    pub dashboard_activity_id: Option<i64>,
    pub date: chrono::NaiveDate,
    pub distance_m: f64,
    pub elevation_gain: f64,
    pub elevation_loss: f64,
    pub steps: i64,
    pub duration_s: f64,
    pub location_name: Option<String>,
}

#[derive(serde::Serialize)]
pub struct TripDetail {
    pub trip: cluster::Trip,
    pub merged: bool,
    pub has_previous: bool,
    pub superlatives: aggregate::Superlatives,
    pub days: Vec<TripDay>,
    pub recovery: Vec<store::RecoveryDay>,
}

pub async fn hiking_trip_detail(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(trip_id): axum::extract::Path<i64>,
) -> Result<Json<TripDetail>, StatusCode> {
    crate::server::ensure_session(&state, &headers)?;
    let (acts, trips) = load_acts_and_trips(&state)?;
    let idx = trips.iter().position(|t| t.id == trip_id).ok_or(StatusCode::NOT_FOUND)?;
    let trip = trips[idx].clone();

    let overrides = load_overrides(&state);
    let merged = trip.activity_ids.iter()
        .any(|id| matches!(overrides.get(id), Some(Override::LinkPrevious)));

    let superlatives = aggregate::superlatives(&trip, &acts);

    let days: Vec<TripDay> = trip.activity_ids.iter().filter_map(|id| {
        acts.iter().find(|a| a.activity_id == *id).map(|a| TripDay {
            garmin_activity_id: a.activity_id,
            dashboard_activity_id: state.db
                .activity_id_by_file_name(&format!("{}_ACTIVITY.fit", a.activity_id))
                .ok().flatten(),
            date: a.date,
            distance_m: a.distance_m,
            elevation_gain: a.elevation_gain,
            elevation_loss: a.elevation_loss,
            steps: a.steps,
            duration_s: a.duration_s,
            location_name: a.location_name.clone(),
        })
    }).collect();

    let path = state.garmin_db_path.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let recovery = store::load_recovery(path, trip.start_date, trip.end_date).map_err(|e| {
        tracing::error!(error = %e, "garmin.db recovery load failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(TripDetail {
        merged,
        has_previous: idx > 0,
        superlatives,
        days,
        recovery,
        trip,
    }))
}
```

- [ ] **Step 3: Merge / split / rename endpoints**

```rust
#[derive(serde::Serialize)]
pub struct MergeResult { pub trip_id: i64 }

pub async fn hiking_merge_previous(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(trip_id): axum::extract::Path<i64>,
) -> Result<Json<MergeResult>, StatusCode> {
    crate::server::ensure_session(&state, &headers)?;
    let (_, trips) = load_acts_and_trips(&state)?;
    let idx = trips.iter().position(|t| t.id == trip_id).ok_or(StatusCode::NOT_FOUND)?;
    if idx == 0 {
        return Err(StatusCode::BAD_REQUEST); // nothing before this trip
    }
    // trip id == first member activity id; linking it joins this trip to the previous one
    state.db.set_hiking_override(trip_id, Some("link_previous")).map_err(|e| {
        tracing::error!(error = %e, "failed to store merge override");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(MergeResult { trip_id: trips[idx - 1].id }))
}

pub async fn hiking_split_trip(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(trip_id): axum::extract::Path<i64>,
) -> Result<Json<MergeResult>, StatusCode> {
    crate::server::ensure_session(&state, &headers)?;
    let (_, trips) = load_acts_and_trips(&state)?;
    let trip = trips.iter().find(|t| t.id == trip_id).ok_or(StatusCode::NOT_FOUND)?;
    let overrides = load_overrides(&state);
    for id in &trip.activity_ids {
        if matches!(overrides.get(id), Some(Override::LinkPrevious)) {
            state.db.set_hiking_override(*id, None).map_err(|e| {
                tracing::error!(error = %e, "failed to remove merge override");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }
    }
    Ok(Json(MergeResult { trip_id }))
}

#[derive(serde::Deserialize)]
pub struct TripNameBody { pub trip_id: i64, pub name: Option<String> }

pub async fn hiking_set_trip_name(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<TripNameBody>,
) -> Result<StatusCode, StatusCode> {
    crate::server::ensure_session(&state, &headers)?;
    let name = body.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    state.db.set_hiking_trip_name(body.trip_id, name).map_err(|e| {
        tracing::error!(error = %e, "failed to store trip name");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}
```

NOTE: adapt extractor order/styles to whatever the existing handlers in this file actually use (headers before/after State etc. — mirror `hiking_overview`). `Json(body)` must be the LAST extractor in axum.

- [ ] **Step 4: Routes**

In `src-tauri/src/server.rs`, after the two existing hiking routes add:

```rust
        .route("/api/hiking/trip/{id}", get(crate::hiking::http::hiking_trip_detail))
        .route("/api/hiking/trip/{id}/merge-previous", post(crate::hiking::http::hiking_merge_previous))
        .route("/api/hiking/trip/{id}/split", post(crate::hiking::http::hiking_split_trip))
        .route("/api/hiking/trip-name", put(crate::hiking::http::hiking_set_trip_name))
```

(Check the file's existing imports for `put` — `axum::routing::{get, post}` likely needs `put` added.)

- [ ] **Step 5: Build + manual verification**

Run: `cargo build --features web --manifest-path src-tauri/Cargo.toml` and `cargo check --features tauri-app --manifest-path src-tauri/Cargo.toml` — both PASS.

Manual check with the real db (throwaway data dir):

```bash
rm -rf /tmp/fit-dash-p2 && mkdir -p /tmp/fit-dash-p2
FIT_DASHBOARD_DATA_DIR=/tmp/fit-dash-p2 FIT_DASHBOARD_GARMIN_DB=/tmp/garmin.db \
  cargo run --features web --manifest-path src-tauri/Cargo.toml &
# onboard, capture token:
T=$(curl -s -X POST localhost:8080/api/onboard -H 'Content-Type: application/json' \
    -d '{"username":"t","password":"t-throwaway"}' | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
# trips now have names:
curl -s "localhost:8080/api/hiking/trips?category=thru_hike" -H "X-Session: $T" | python3 -m json.tool | head -20
# detail of the 55n PCT segment (find its id from the list; it is the trip with start_date 2022-04-10):
ID=$(curl -s "localhost:8080/api/hiking/trips?category=thru_hike" -H "X-Session: $T" | python3 -c 'import sys,json;print([t["id"] for t in json.load(sys.stdin) if t["start_date"]=="2022-04-10"][0])')
curl -s "localhost:8080/api/hiking/trip/$ID" -H "X-Session: $T" | python3 -c 'import sys,json;d=json.load(sys.stdin);print("days:",len(d["days"]),"recovery:",len(d["recovery"]),"name:",d["trip"]["name"],"superl:",d["superlatives"])'
# merge the 2022-06-07 segment into the 55n one and verify the merged trip:
ID2=$(curl -s "localhost:8080/api/hiking/trips?category=thru_hike" -H "X-Session: $T" | python3 -c 'import sys,json;print([t["id"] for t in json.load(sys.stdin) if t["start_date"]=="2022-06-07"][0])')
curl -s -X POST "localhost:8080/api/hiking/trip/$ID2/merge-previous" -H "X-Session: $T"
curl -s "localhost:8080/api/hiking/trip/$ID" -H "X-Session: $T" | python3 -c 'import sys,json;d=json.load(sys.stdin);print("merged:",d["merged"],"end:",d["trip"]["end_date"],"nights:",d["trip"]["nights"])'
# split it again (leave a clean state), rename check:
curl -s -X POST "localhost:8080/api/hiking/trip/$ID/split" -H "X-Session: $T"
curl -s -X PUT "localhost:8080/api/hiking/trip-name" -H "X-Session: $T" -H 'Content-Type: application/json' -d "{\"trip_id\": $ID, \"name\": \"PCT test\"}"
curl -s "localhost:8080/api/hiking/trips?category=thru_hike&year=2022" -H "X-Session: $T" | python3 -c 'import sys,json;print([t["name"] for t in json.load(sys.stdin)])'
curl -s -X PUT "localhost:8080/api/hiking/trip-name" -H "X-Session: $T" -H 'Content-Type: application/json' -d "{\"trip_id\": $ID, \"name\": null}"
kill %1
```

Expected: detail returns ~50 day entries (one per member hike) + exactly 56 recovery entries (Apr 10 – Jun 4 inclusive) and a county-based name; after merge `merged: true`, `end: 2022-06-12`, nights 63; after split back to 5 thru-hikes; rename round-trips. Paste actual outputs in your report.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/hiking/http.rs src-tauri/src/server.rs
git commit -m "feat(hiking): trip detail, merge/split and rename endpoints"
```

---

### Task 5: Frontend API client + types

**Files:**
- Modify: `src/types.ts`
- Modify: `src/lib/api.ts`

- [ ] **Step 1: Types**

In `src/types.ts`: add `name: string | null;` to the existing `Trip` type (after `category`), and append:

```ts
export type TripDay = {
  garmin_activity_id: number;
  dashboard_activity_id: number | null;
  date: string;
  distance_m: number;
  elevation_gain: number;
  elevation_loss: number;
  steps: number;
  duration_s: number;
  location_name: string | null;
};

export type RecoveryDay = {
  date: string;
  sleep_score: number | null;
  sleep_seconds: number | null;
  resting_hr: number | null;
  hrv_last_night_avg: number | null;
  body_battery_high: number | null;
  body_battery_low: number | null;
  avg_stress: number | null;
  training_readiness: number | null;
};

export type Superlatives = {
  longest_day_m: number;
  biggest_climb_m: number;
  highest_point_m: number | null;
};

export type TripDetail = {
  trip: Trip;
  merged: boolean;
  has_previous: boolean;
  superlatives: Superlatives;
  days: TripDay[];
  recovery: RecoveryDay[];
};
```

- [ ] **Step 2: Client functions**

In `src/lib/api.ts`, next to the existing hiking functions (same `isTauriRuntime()` guard as those, same `webClient` usage):

```ts
  async hikingTrip(tripId: number): Promise<TripDetail> {
    if (isTauriRuntime()) throw new Error("hiking features are only available in web mode");
    return (await webClient.get(`/hiking/trip/${tripId}`)).data;
  },
  async hikingMergePrevious(tripId: number): Promise<{ trip_id: number }> {
    if (isTauriRuntime()) throw new Error("hiking features are only available in web mode");
    return (await webClient.post(`/hiking/trip/${tripId}/merge-previous`)).data;
  },
  async hikingSplitTrip(tripId: number): Promise<{ trip_id: number }> {
    if (isTauriRuntime()) throw new Error("hiking features are only available in web mode");
    return (await webClient.post(`/hiking/trip/${tripId}/split`)).data;
  },
  async hikingSetTripName(tripId: number, name: string | null): Promise<void> {
    if (isTauriRuntime()) throw new Error("hiking features are only available in web mode");
    await webClient.put(`/hiking/trip-name`, { trip_id: tripId, name });
  },
```

Import the new types. Mirror the file's actual axios instance/guard patterns exactly (read the existing hiking functions first).

- [ ] **Step 3: Typecheck + commit**

Run: `npx tsc --noEmit` — PASS.

```bash
git add src/types.ts src/lib/api.ts
git commit -m "feat(hiking): trip detail api client + types"
```

---

### Task 6: Shared map style + TripMap component

**Files:**
- Create: `src/lib/mapStyle.ts`
- Modify: `src/components/ActivityMap.tsx` (import from the new module; no behavior change)
- Create: `src/components/TripMap.tsx`

- [ ] **Step 1: Extract basemap helpers**

Move `BASEMAPS` and `styleFromMap` (and the `BaseMapInfo` type) from `ActivityMap.tsx` into a new `src/lib/mapStyle.ts`, exporting all three. Update `ActivityMap.tsx` to import them (`import { BASEMAPS, styleFromMap } from "../lib/mapStyle";`) and delete the moved definitions. Move ONLY these — nothing else from ActivityMap. If `styleFromMap` references other locals, move the minimal closure of what it needs.

Run: `npx tsc --noEmit` and `npm run build` — both PASS (pure refactor).

- [ ] **Step 2: TripMap component**

Create `src/components/TripMap.tsx`:

```tsx
import { useEffect, useRef } from "react";
import maplibregl from "maplibre-gl";
import type { RecordPoint } from "../types";
import { useSettingsStore } from "../stores/settingsStore";
import { styleFromMap } from "../lib/mapStyle";

type Props = { tracks: RecordPoint[][] };

/** Stitched multi-day route map: one line per day's activity. */
export function TripMap({ tracks }: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const theme = useSettingsStore((s) => s.theme);
  const mapStyle = useSettingsStore((s) => s.mapStyle);

  useEffect(() => {
    if (!containerRef.current) return;
    const coordsPerTrack = tracks
      .map((t) => t.filter((p) => p.latitude != null && p.longitude != null)
        .map((p) => [p.longitude as number, p.latitude as number]))
      .filter((c) => c.length > 1);
    if (coordsPerTrack.length === 0) return;

    const map = new maplibregl.Map({
      container: containerRef.current,
      style: styleFromMap(mapStyle, theme === "dark" ? "dark" : "light"),
      attributionControl: { compact: true },
    });
    mapRef.current = map;
    map.addControl(new maplibregl.NavigationControl({ showCompass: false }));

    map.on("load", () => {
      map.addSource("trip", {
        type: "geojson",
        data: {
          type: "Feature",
          properties: {},
          geometry: { type: "MultiLineString", coordinates: coordsPerTrack },
        },
      });
      map.addLayer({
        id: "trip-line",
        type: "line",
        source: "trip",
        paint: { "line-color": "#2f7fd1", "line-width": 3 },
        layout: { "line-cap": "round", "line-join": "round" },
      });
      const bounds = new maplibregl.LngLatBounds();
      for (const track of coordsPerTrack) for (const c of track) bounds.extend(c as [number, number]);
      map.fitBounds(bounds, { padding: 40, animate: false });
    });

    return () => { map.remove(); mapRef.current = null; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tracks, mapStyle, theme]);

  if (tracks.every((t) => !t.some((p) => p.latitude != null))) return null;
  return <div className="trip-map" ref={containerRef} />;
}
```

Adapt to the real `settingsStore` shape (check how ActivityMap reads theme/mapStyle — mirror it; if ActivityMap receives them as props instead, read the store the way `Dashboard.tsx` does and keep TripMap self-sufficient via the store). Check `styleFromMap`'s real signature from Step 1 and match it.

Add to `src/styles.css` (scoped, reusing tokens):

```css
.hiking-tab .trip-map {
  height: 380px;
  border-radius: 12px;
  overflow: hidden;
  border: 1px solid var(--border);
}
```

- [ ] **Step 3: Build + commit**

Run: `npx tsc --noEmit` and `npm run build` — PASS.

```bash
git add src/lib/mapStyle.ts src/components/ActivityMap.tsx src/components/TripMap.tsx src/styles.css
git commit -m "feat(hiking): stitched trip map component + shared basemap styles"
```

---

### Task 7: Trip detail view in the Hiking tab + day → activity link

**Files:**
- Create: `src/components/HikingTripDetail.tsx`
- Modify: `src/components/HikingTab.tsx` (trip names in rows, row click → detail)
- Modify: `src/components/Dashboard.tsx` (pass `onOpenActivity`)
- Modify: `src/styles.css`

- [ ] **Step 1: Detail component**

Create `src/components/HikingTripDetail.tsx`:

```tsx
import { useCallback, useEffect, useState } from "react";
import ReactECharts from "echarts-for-react";
import { api } from "../lib/api";
import type { RecordPoint, RecoveryDay, TripDetail } from "../types";
import { TripMap } from "./TripMap";

const KM = (m: number) => (m / 1000).toFixed(0);

type Props = {
  tripId: number;
  onBack: () => void;
  onTripChanged: (newTripId: number) => void; // after merge/split, navigate + refetch lists
  onOpenActivity?: (dashboardActivityId: number) => void;
};

function recoveryChart(recovery: RecoveryDay[], series: Array<{ key: keyof RecoveryDay; label: string }>, title: string) {
  return {
    title: { text: title, textStyle: { fontSize: 12 }, left: 4, top: 2 },
    tooltip: { trigger: "axis" },
    grid: { left: 36, right: 12, top: 30, bottom: 20 },
    xAxis: { type: "category", data: recovery.map((r) => r.date.slice(5)), axisLabel: { fontSize: 9 } },
    yAxis: { type: "value", axisLabel: { fontSize: 9 }, scale: true },
    series: series.map((s) => ({
      name: s.label,
      type: "line",
      connectNulls: true,
      showSymbol: false,
      data: recovery.map((r) => r[s.key] as number | null),
    })),
    legend: series.length > 1 ? { bottom: 0, textStyle: { fontSize: 9 } } : undefined,
  };
}

export function HikingTripDetail({ tripId, onBack, onTripChanged, onOpenActivity }: Props) {
  const [detail, setDetail] = useState<TripDetail | null>(null);
  const [tracks, setTracks] = useState<RecordPoint[][]>([]);
  const [editingName, setEditingName] = useState(false);
  const [nameDraft, setNameDraft] = useState("");
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    let cancelled = false;
    api.hikingTrip(tripId).then((d) => {
      if (cancelled) return;
      setDetail(d);
      const ids = d.days.map((day) => day.dashboard_activity_id).filter((id): id is number => id != null);
      void Promise.all(ids.map((id) => api.getRecords(id, 30000).catch((): RecordPoint[] => [])))
        .then((ts) => { if (!cancelled) setTracks(ts.filter((t) => t.length > 0)); });
    }).catch(() => { if (!cancelled) setDetail(null); });
    return () => { cancelled = true; };
  }, [tripId]);

  useEffect(() => load(), [load]);

  if (!detail) return <div className="hiking-tab"><button className="hiking-back" onClick={onBack}>← Back</button><p className="empty">Loading…</p></div>;

  const t = detail.trip;
  const title = t.name ?? `${t.start_date}${t.nights > 0 ? ` → ${t.end_date}` : ""}`;

  async function saveName() {
    const name = nameDraft.trim();
    await api.hikingSetTripName(t.id, name.length > 0 ? name : null);
    setEditingName(false);
    load();
  }

  async function mergePrevious() {
    setBusy(true);
    try {
      const r = await api.hikingMergePrevious(t.id);
      onTripChanged(r.trip_id);
    } finally { setBusy(false); }
  }

  async function splitTrip() {
    setBusy(true);
    try {
      const r = await api.hikingSplitTrip(t.id);
      onTripChanged(r.trip_id);
    } finally { setBusy(false); }
  }

  return (
    <div className="hiking-tab hiking-detail">
      <div className="hiking-detail-header">
        <button className="hiking-back" onClick={onBack}>← Back</button>
        {editingName ? (
          <span className="hiking-name-edit">
            <input value={nameDraft} onChange={(e) => setNameDraft(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") void saveName(); if (e.key === "Escape") setEditingName(false); }}
              placeholder="Trip name" autoFocus />
            <button onClick={() => void saveName()}>Save</button>
            <button onClick={() => setEditingName(false)}>Cancel</button>
          </span>
        ) : (
          <h2>
            {title}
            <button className="hiking-rename" title="Rename trip"
              onClick={() => { setNameDraft(t.name ?? ""); setEditingName(true); }}>✎</button>
          </h2>
        )}
        <span className="hiking-detail-actions">
          {detail.has_previous && (
            <button disabled={busy} onClick={() => void mergePrevious()} title="Join this trip onto the trip before it">
              Merge into previous trip
            </button>
          )}
          {detail.merged && (
            <button disabled={busy} onClick={() => void splitTrip()} title="Undo manual merges inside this trip">
              Split merged trips
            </button>
          )}
        </span>
      </div>

      <div className="hiking-stats">
        <div className="stat-card"><div className="stat-value">{t.start_date}{t.nights > 0 ? ` → ${t.end_date}` : ""}</div><div className="stat-label">{t.nights} nights · {t.activity_ids.length} hikes</div></div>
        <div className="stat-card"><div className="stat-value">{KM(t.total_distance_m)} km</div><div className="stat-label">distance</div></div>
        <div className="stat-card"><div className="stat-value">+{Math.round(t.total_gain).toLocaleString()} m</div><div className="stat-label">elev gain</div></div>
        <div className="stat-card"><div className="stat-value">-{Math.round(t.total_loss).toLocaleString()} m</div><div className="stat-label">elev loss</div></div>
        <div className="stat-card"><div className="stat-value">{t.total_steps.toLocaleString()}</div><div className="stat-label">steps</div></div>
      </div>

      <div className="hiking-superlatives">
        <span>Longest day: <b>{(detail.superlatives.longest_day_m / 1000).toFixed(1)} km</b></span>
        <span>Biggest climb: <b>▲{Math.round(detail.superlatives.biggest_climb_m)} m</b></span>
        {detail.superlatives.highest_point_m != null && (
          <span>Highest point: <b>{Math.round(detail.superlatives.highest_point_m)} m</b></span>
        )}
      </div>

      {tracks.length > 0 && <TripMap tracks={tracks} />}

      <section className="hiking-section panel">
        <h3>Days</h3>
        {detail.days.map((d) => (
          <div key={d.garmin_activity_id}
            className={`trip-row${d.dashboard_activity_id != null ? " clickable" : ""}`}
            onClick={() => { if (d.dashboard_activity_id != null) onOpenActivity?.(d.dashboard_activity_id); }}>
            <span className="trip-dates">{d.date}</span>
            <span className="trip-location">{d.location_name ?? ""}</span>
            <span className="trip-km">{(d.distance_m / 1000).toFixed(1)} km</span>
            <span className="trip-gain">▲{Math.round(d.elevation_gain)}</span>
          </div>
        ))}
      </section>

      <div className="hiking-recovery">
        <div className="panel"><ReactECharts option={recoveryChart(detail.recovery, [{ key: "sleep_score", label: "Sleep score" }], "Sleep score")} style={{ height: 180 }} /></div>
        <div className="panel"><ReactECharts option={recoveryChart(detail.recovery, [{ key: "resting_hr", label: "Resting HR" }, { key: "hrv_last_night_avg", label: "HRV" }], "Resting HR + HRV")} style={{ height: 180 }} /></div>
        <div className="panel"><ReactECharts option={recoveryChart(detail.recovery, [{ key: "body_battery_high", label: "BB high" }, { key: "body_battery_low", label: "BB low" }, { key: "avg_stress", label: "Stress" }], "Body Battery + Stress")} style={{ height: 180 }} /></div>
        <div className="panel"><ReactECharts option={recoveryChart(detail.recovery, [{ key: "training_readiness", label: "Readiness" }], "Training readiness")} style={{ height: 180 }} /></div>
      </div>
    </div>
  );
}
```

This is the reviewed spec for structure/behavior; adapt surface details (theme handling for ECharts, existing class reuse) to sibling components. Keep the race-safe load pattern.

- [ ] **Step 2: Wire into HikingTab**

In `src/components/HikingTab.tsx`:
- Add prop type: `type Props = { onOpenActivity?: (dashboardActivityId: number) => void };` and accept it: `export function HikingTab({ onOpenActivity }: Props)`.
- Add state: `const [selectedTripId, setSelectedTripId] = useState<number | null>(null);` and a refresh key so lists refetch after merge/split: extend the fetch `useEffect` deps to `[year, refreshKey]` with `const [refreshKey, setRefreshKey] = useState(0);`.
- Early-return the detail view before the list rendering:

```tsx
  if (selectedTripId != null) {
    return (
      <HikingTripDetail
        tripId={selectedTripId}
        onBack={() => { setSelectedTripId(null); setRefreshKey((k) => k + 1); }}
        onTripChanged={(id) => { setSelectedTripId(id); setRefreshKey((k) => k + 1); }}
        onOpenActivity={onOpenActivity}
      />
    );
  }
```

- Make trip rows clickable and show the name: on the `.trip-row` div add `onClick={() => setSelectedTripId(t.id)}` and `className="trip-row clickable"`, and insert a name span before the dates:

```tsx
              <span className="trip-name">{t.name ?? "—"}</span>
```

- Import `HikingTripDetail`.

- [ ] **Step 3: Dashboard link**

In `src/components/Dashboard.tsx`, where `<HikingTab />` is rendered, pass:

```tsx
<HikingTab onOpenActivity={(id) => {
  const a = activities.find((x) => x.id === id);
  if (a) { void selectActivity(a); setTab("individual"); }
}} />
```

(`activities` and `selectActivity` already come from the activity store destructure at ~line 332-333.)

- [ ] **Step 4: Styles**

Append to `src/styles.css` (all scoped under `.hiking-tab`, reuse tokens):

```css
.hiking-tab .trip-row.clickable { cursor: pointer; }
.hiking-tab .trip-row.clickable:hover { background: var(--input-bg); }
.hiking-tab .trip-name { flex: 1; font-weight: 600; color: var(--text); }
.hiking-tab .trip-location { flex: 1; color: var(--text-muted); }
.hiking-tab .hiking-detail-header { display: flex; align-items: center; gap: 12px; margin-bottom: 14px; flex-wrap: wrap; }
.hiking-tab .hiking-detail-header h2 { margin: 0; font-size: 20px; }
.hiking-tab .hiking-back { background: none; border: 1px solid var(--border); border-radius: 8px; padding: 4px 10px; color: var(--text-muted); cursor: pointer; }
.hiking-tab .hiking-rename { background: none; border: none; color: var(--text-muted); cursor: pointer; margin-left: 6px; }
.hiking-tab .hiking-detail-actions { margin-left: auto; display: flex; gap: 8px; }
.hiking-tab .hiking-detail-actions button { border: 1px solid var(--border); background: var(--input-bg); border-radius: 8px; padding: 4px 10px; cursor: pointer; color: var(--text); }
.hiking-tab .hiking-name-edit { display: flex; gap: 6px; align-items: center; }
.hiking-tab .hiking-superlatives { display: flex; gap: 18px; margin: 10px 0 14px; color: var(--text-muted); }
.hiking-tab .hiking-recovery { display: grid; grid-template-columns: repeat(2, 1fr); gap: 12px; margin-top: 14px; }
```

Adjust variable names to those that actually exist in styles.css (check `--text`, `--input-bg` etc. against Phase 1's hiking styles which already use real tokens).

- [ ] **Step 5: Build + commit**

Run: `npx tsc --noEmit` and `npm run build` — PASS.

```bash
git add src/components/HikingTripDetail.tsx src/components/HikingTab.tsx src/components/Dashboard.tsx src/styles.css
git commit -m "feat(hiking): trip detail view with map, recovery trends, rename and merge"
```

---

### Task 8: End-to-end verification + deploy + PCT merge

**Files:** none (verification/deployment; controller-driven)

- [ ] **Step 1: Local browser verification** — run backend (`FIT_DASHBOARD_DATA_DIR=<throwaway> FIT_DASHBOARD_GARMIN_DB=/tmp/garmin.db cargo run --features web ...`) + `npm run dev`; in the browser: trip rows show names; clicking a thru-hike opens the detail (map with stitched track, superlatives, days, 4 recovery charts); day row click jumps to the Individual view of that activity; rename round-trips; merge/split works.
- [ ] **Step 2: Deploy to CT 115** — same path as Phase 1: `git -C /root/fit-dashboard-build pull`, `docker build -f docker/Dockerfile -t fit-dashboard:hiking-phase2 .`, swap `image:` in `/opt/appdata/fitness/docker-compose.yml`, `docker compose up -d`, verify endpoints with the bridge env.
- [ ] **Step 3: Merge the PCT** — in the live UI (or via the merge endpoint), merge the four later 2022 PCT segments into the first: open each later segment (06-07, 06-16, 08-07, 08-14 start dates) and click "Merge into previous trip". Expected end state: ONE 2022 thru-hike 2022-04-10 → 2022-09-12 (155 nights), and the year stats unchanged (totals are override-invariant). Rename it "PCT 2022".

---

## Notes for the executor

- Engine changes (Task 1) are strict TDD; storage (Task 2) and loader (Task 3) each carry their own tests; HTTP/frontend tasks (4–7) are wiring — mirror the existing patterns from Phase 1 files, don't invent new ones.
- `trip_id` = first member's garmin `activity_id` (stable). Merging trip B into A stores `link_previous` on B's first activity; the combined trip keeps A's id, so A's name override survives merges.
- Keep `HikingSettings::default()` as the settings seam (full settings UI remains Phase 3).
- All new endpoints go through `ensure_session` per-handler like Phase 1; http.rs stays cfg-gated web-only; always finish backend tasks with `cargo check --features tauri-app`.
