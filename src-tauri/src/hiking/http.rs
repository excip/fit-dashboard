use std::collections::HashMap;
use std::sync::Arc;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use crate::state::AppState;
use crate::hiking::{HikingSettings, store, cluster, aggregate};
use crate::server::ensure_session;

#[derive(serde::Deserialize)]
pub struct YearQuery {
    pub year: Option<i32>,
}

fn load_trips(state: &AppState) -> Result<Vec<cluster::Trip>, StatusCode> {
    let path = state.garmin_db_path.as_ref().ok_or_else(|| {
        tracing::warn!("hiking endpoint unavailable: garmin db not configured");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    // re-read per request is ~10ms on real data; caching deferred until Phase 2
    let acts = store::load_activities(path).map_err(|e| {
        tracing::error!(error = %e, "garmin.db load failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let settings = HikingSettings::default();          // Phase 3: load from DuckDB
    let overrides = HashMap::new();                     // Phase 3: load from DuckDB
    Ok(cluster::cluster_trips(&acts, &settings, &overrides))
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
