use std::collections::HashMap;
use std::sync::Arc;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use crate::state::AppState;
use crate::hiking::{HikingSettings, Override, store, cluster, aggregate, baseline};
use crate::server::ensure_session;

#[derive(serde::Deserialize)]
pub struct YearQuery {
    pub year: Option<i32>,
}

fn parse_override(kind: &str) -> Option<Override> {
    match kind {
        "force_hike" => Some(Override::ForceHike),
        "force_walk" => Some(Override::ForceWalk),
        "link_previous" => Some(Override::LinkPrevious),
        _ => None,
    }
}

fn load_overrides(state: &AppState) -> Result<HashMap<i64, Override>, StatusCode> {
    state.db.hiking_overrides().map_err(|e| {
        tracing::error!(error = %e, "failed to load hiking overrides");
        StatusCode::INTERNAL_SERVER_ERROR
    }).map(|rows| {
        rows.into_iter()
            .filter_map(|(id, k)| parse_override(&k).map(|o| (id, o)))
            .collect()
    })
}

struct HikingData {
    acts: Vec<crate::hiking::HikeActivity>,
    trips: Vec<cluster::Trip>,
    overrides: HashMap<i64, Override>,
    garmin_db_path: std::sync::Arc<std::path::PathBuf>,
}

fn load_acts_and_trips(state: &AppState) -> Result<HikingData, StatusCode> {
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
    let overrides = load_overrides(state)?;
    let mut trips = cluster::cluster_trips(&acts, &settings, &overrides);
    for (id, name) in state.db.hiking_trip_names().map_err(|e| {
        tracing::error!(error = %e, "failed to load hiking trip names");
        StatusCode::INTERNAL_SERVER_ERROR
    })? {
        if let Some(t) = trips.iter_mut().find(|t| t.id == id) {
            t.name = Some(name);
        }
    }
    Ok(HikingData { acts, trips, overrides, garmin_db_path: path.clone() })
}

fn load_trips(state: &AppState) -> Result<Vec<cluster::Trip>, StatusCode> {
    Ok(load_acts_and_trips(state)?.trips)
}

pub async fn hiking_overview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<YearQuery>,
) -> Result<Json<aggregate::OverviewStats>, StatusCode> {
    ensure_session(&state, &headers)?;
    let trips = load_trips(&state)?;
    Ok(Json(aggregate::overview(&trips, q.year)))
}

#[derive(serde::Deserialize)]
pub struct CategoryQuery {
    pub category: Option<String>,
    pub year: Option<i32>,
}

pub async fn hiking_trips(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<CategoryQuery>,
) -> Result<Json<Vec<cluster::Trip>>, StatusCode> {
    use chrono::Datelike;
    ensure_session(&state, &headers)?;
    let mut trips = load_trips(&state)?;
    if let Some(y) = q.year {
        trips.retain(|t| t.start_date.year() == y);
    }
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
    pub baselines: baseline::Baselines,
    pub recovery_summary: baseline::RecoverySummary,
}

pub async fn hiking_trip_detail(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(trip_id): axum::extract::Path<i64>,
) -> Result<Json<TripDetail>, StatusCode> {
    ensure_session(&state, &headers)?;
    let HikingData { acts, trips, overrides, garmin_db_path } = load_acts_and_trips(&state)?;
    let idx = trips.iter().position(|t| t.id == trip_id).ok_or(StatusCode::NOT_FOUND)?;
    let trip = trips[idx].clone();

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
}

#[derive(serde::Serialize)]
pub struct MergeResult { pub trip_id: i64 }

pub async fn hiking_merge_previous(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(trip_id): axum::extract::Path<i64>,
) -> Result<Json<MergeResult>, StatusCode> {
    ensure_session(&state, &headers)?;
    let trips = load_acts_and_trips(&state)?.trips;
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
    headers: HeaderMap,
    axum::extract::Path(trip_id): axum::extract::Path<i64>,
) -> Result<Json<MergeResult>, StatusCode> {
    ensure_session(&state, &headers)?;
    let HikingData { trips, overrides, .. } = load_acts_and_trips(&state)?;
    let trip = trips.iter().find(|t| t.id == trip_id).ok_or(StatusCode::NOT_FOUND)?;
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
    headers: HeaderMap,
    Json(body): Json<TripNameBody>,
) -> Result<StatusCode, StatusCode> {
    ensure_session(&state, &headers)?;
    let name = body.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    state.db.set_hiking_trip_name(body.trip_id, name).map_err(|e| {
        tracing::error!(error = %e, "failed to store trip name");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}
