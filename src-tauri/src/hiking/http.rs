use std::collections::HashMap;
use std::sync::Arc;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use crate::database::GeneratedNote;
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

pub(crate) struct HikingData {
    pub(crate) acts: Vec<crate::hiking::HikeActivity>,
    pub(crate) trips: Vec<cluster::Trip>,
    pub(crate) overrides: HashMap<i64, Override>,
    pub(crate) garmin_db_path: std::sync::Arc<std::path::PathBuf>,
}

pub(crate) fn load_acts_and_trips(state: &AppState) -> Result<HikingData, StatusCode> {
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

/// A trip row plus its manual rating (additive over the flattened `Trip` fields).
#[derive(serde::Serialize)]
pub struct TripRow {
    #[serde(flatten)]
    pub trip: cluster::Trip,
    pub user_rating: Option<i64>,
}

pub async fn hiking_trips(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<CategoryQuery>,
) -> Result<Json<Vec<TripRow>>, StatusCode> {
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

    let ratings: HashMap<i64, i64> = state.db.hiking_user_notes().map_err(|e| {
        tracing::error!(error = %e, "failed to load hiking user notes");
        StatusCode::INTERNAL_SERVER_ERROR
    })?.into_iter()
        .filter(|n| n.subject_type == "trip")
        .filter_map(|n| n.rating.map(|r| (n.subject_id, r)))
        .collect();

    let rows: Vec<TripRow> = trips.into_iter()
        .map(|t| TripRow { user_rating: ratings.get(&t.id).copied(), trip: t })
        .collect();
    Ok(Json(rows))
}

/// One activity's simplified GPS line for the atlas map, chronological order.
#[derive(serde::Serialize)]
pub struct AtlasTrack {
    pub trip_id: i64,
    pub activity_id: i64,
    pub date: chrono::NaiveDate,
    pub distance_m: f64,
    /// [lon, lat] pairs, RDP-simplified and rounded to ~1 m precision.
    pub coords: Vec<[f64; 2]>,
}

/// All hiking tracks in one payload, drawn by the atlas overview map.
/// 60 s time buckets + ~17 m RDP tolerance keep it around 1 MB for a
/// decade of hiking while staying visually faithful at overview zooms.
pub async fn hiking_tracks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<YearQuery>,
) -> Result<Json<Vec<AtlasTrack>>, StatusCode> {
    use chrono::Datelike;
    ensure_session(&state, &headers)?;
    let HikingData { acts, mut trips, .. } = load_acts_and_trips(&state)?;
    if let Some(y) = q.year {
        trips.retain(|t| t.start_date.year() == y);
    }

    let mut out: Vec<AtlasTrack> = Vec::new();
    for trip in &trips {
        for gid in &trip.activity_ids {
            let Some(act) = acts.iter().find(|a| a.activity_id == *gid) else { continue };
            let Some(dash_id) = state.db
                .activity_id_by_file_name(&format!("{gid}_ACTIVITY.fit"))
                .ok()
                .flatten()
            else { continue };
            let pts = state.db.track_points(dash_id, 60_000).map_err(|e| {
                tracing::error!(error = %e, activity_id = dash_id, "track points load failed");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            if pts.len() < 2 {
                continue;
            }
            let coords: Vec<[f64; 2]> = crate::hiking::simplify::simplify_track(&pts, 0.00015)
                .into_iter()
                .map(|(lon, lat)| {
                    [crate::hiking::simplify::round5(lon), crate::hiking::simplify::round5(lat)]
                })
                .collect();
            out.push(AtlasTrack {
                trip_id: trip.id,
                activity_id: *gid,
                date: act.date,
                distance_m: act.distance_m,
                coords,
            });
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date).then(a.activity_id.cmp(&b.activity_id)));
    tracing::debug!(tracks = out.len(), "hiking tracks completed");
    Ok(Json(out))
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
    pub custom_name: Option<String>,
    pub generated_note: Option<String>,
    pub user_note: Option<String>,
    pub user_rating: Option<i64>,
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
    pub generated_note: Option<String>,
    pub generated_note_model: Option<String>,
    pub generated_note_date: Option<String>,
    pub user_note: Option<String>,
    pub user_rating: Option<i64>,
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

    let activity_names: HashMap<i64, String> = state.db.hiking_activity_names().map_err(|e| {
        tracing::error!(error = %e, "failed to load hiking activity names");
        StatusCode::INTERNAL_SERVER_ERROR
    })?.into_iter().collect();

    // (subject_type, subject_id) -> (note, rating) and -> generated note text,
    // fetched once and shared by the trip and each per-day lookup.
    let user_notes: HashMap<(String, i64), (Option<String>, Option<i64>)> =
        state.db.hiking_user_notes().map_err(|e| {
            tracing::error!(error = %e, "failed to load hiking user notes");
            StatusCode::INTERNAL_SERVER_ERROR
        })?.into_iter()
        .map(|n| ((n.subject_type, n.subject_id), (n.note, n.rating)))
        .collect();
    let generated_notes: HashMap<(String, i64), GeneratedNote> =
        state.db.hiking_generated_notes().map_err(|e| {
            tracing::error!(error = %e, "failed to load hiking generated notes");
            StatusCode::INTERNAL_SERVER_ERROR
        })?.into_iter()
        .map(|g| ((g.subject_type.clone(), g.subject_id), g))
        .collect();

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
            custom_name: activity_names.get(&a.activity_id).cloned(),
            generated_note: generated_notes
                .get(&("activity".to_string(), a.activity_id))
                .map(|g| g.note.clone()),
            user_note: user_notes
                .get(&("activity".to_string(), a.activity_id))
                .and_then(|(n, _)| n.clone()),
            user_rating: user_notes
                .get(&("activity".to_string(), a.activity_id))
                .and_then(|(_, r)| *r),
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

    let trip_key = ("trip".to_string(), trip.id);
    let trip_generated_note = generated_notes.get(&trip_key);
    let generated_note = trip_generated_note.map(|g| g.note.clone());
    let generated_note_model = trip_generated_note.map(|g| g.model.clone());
    let generated_note_date = trip_generated_note.and_then(|g| {
        chrono::NaiveDateTime::parse_from_str(&g.generated_at, "%Y-%m-%d %H:%M:%S%.f")
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(&g.generated_at, "%Y-%m-%d %H:%M:%S")
            })
            .ok()
            .map(|dt| dt.format("%b %-d").to_string())
    });
    let (user_note, user_rating) = user_notes
        .get(&trip_key)
        .map(|(n, r)| (n.clone(), *r))
        .unwrap_or((None, None));

    Ok(Json(TripDetail {
        merged,
        has_previous: idx > 0,
        superlatives,
        days,
        recovery,
        baselines,
        recovery_summary,
        generated_note,
        generated_note_model,
        generated_note_date,
        user_note,
        user_rating,
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

#[derive(serde::Deserialize)]
pub struct ActivityNameBody { pub activity_id: i64, pub name: Option<String> }

pub async fn hiking_set_activity_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ActivityNameBody>,
) -> Result<StatusCode, StatusCode> {
    ensure_session(&state, &headers)?;
    let name = body.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    state.db.set_hiking_activity_name(body.activity_id, name).map_err(|e| {
        tracing::error!(error = %e, "failed to store activity name");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
pub struct UserNoteBody {
    pub subject_type: String,
    pub subject_id: i64,
    pub note: Option<String>,
    pub rating: Option<i64>,
}

pub async fn hiking_set_user_note(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<UserNoteBody>,
) -> Result<StatusCode, StatusCode> {
    ensure_session(&state, &headers)?;
    if body.subject_type != "trip" && body.subject_type != "activity" {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    if let Some(r) = body.rating {
        if !(1..=5).contains(&r) {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
    }
    let note = body.note.as_deref().map(str::trim).filter(|s| !s.is_empty());
    state.db.set_hiking_user_note(&body.subject_type, body.subject_id, note, body.rating)
        .map_err(|e| {
            tracing::error!(error = %e, "failed to store user note");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn hiking_notes_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<serde_json::Value>), StatusCode> {
    ensure_session(&state, &headers)?;
    if crate::hiking::notes_pipeline::llm_config().is_none() {
        return Err(StatusCode::CONFLICT);
    }
    let owned = (*state).clone();
    tokio::spawn(async move {
        crate::hiking::notes_pipeline::run_notes(&owned).await;
    });
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({ "started": true }))))
}

pub async fn hiking_notes_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    ensure_session(&state, &headers)?;
    let cfg = crate::hiking::notes_pipeline::llm_config();
    let last_runs = state.db.last_hiking_notes_runs(5).map_err(|e| {
        tracing::error!(error = %e, "failed to load hiking notes runs");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let last_runs: Vec<serde_json::Value> = last_runs.into_iter().map(|r| serde_json::json!({
        "run_at": r.run_at,
        "ok": r.ok,
        "generated": r.generated,
        "skipped": r.skipped,
        "deleted": r.deleted,
        "message": r.message,
    })).collect();
    let stale = crate::hiking::notes_pipeline::stale_count(&state).unwrap_or(0);
    Ok(Json(serde_json::json!({
        "enabled": cfg.is_some(),
        "model": cfg.map(|c| c.model),
        "running": crate::hiking::notes_pipeline::is_running(),
        "last_runs": last_runs,
        "stale": stale,
    })))
}
