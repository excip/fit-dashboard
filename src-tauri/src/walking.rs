//! Walking endpoints: everyday steps and walks (as opposed to hikes), plus the
//! recurring "home loop" — walks that start and end at the front door.
//! Web-only; reads garmin.db read-only like the hiking module.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{Datelike, NaiveDate};
use rusqlite::Connection;

use crate::hiking::{classify, haversine_m, store, HikeActivity, HikingSettings, Override};
use crate::server::ensure_session;
use crate::state::AppState;

/// Start/end must both be within this radius of home to count as the loop.
const LOOP_HOME_RADIUS_M: f64 = 600.0;
const LOOP_MIN_DISTANCE_M: f64 = 2_000.0;
const LOOP_MAX_DISTANCE_M: f64 = 12_000.0;
/// A day of >= this many steps extends the streak record.
const STREAK_MIN_STEPS: i64 = 10_000;

#[derive(serde::Serialize)]
pub struct YearRow {
    pub year: i32,
    pub steps: i64,
    pub distance_m: f64,
    pub active_days: i64,
    pub walks: i64,
    pub walk_distance_m: f64,
    pub walk_steps: i64,
}

#[derive(serde::Serialize)]
pub struct PeriodStats {
    pub steps: i64,
    pub distance_m: f64,
    pub walks: i64,
}

#[derive(serde::Serialize)]
pub struct WalkingRecords {
    pub best_day_steps: i64,
    pub best_day_date: Option<NaiveDate>,
    pub longest_walk_m: f64,
    pub longest_walk_date: Option<NaiveDate>,
    pub longest_streak_days: i64,
    pub longest_streak_end: Option<NaiveDate>,
}

#[derive(serde::Serialize)]
pub struct WalkingOverview {
    pub years: Vec<YearRow>,
    pub week: PeriodStats,
    pub month: PeriodStats,
    pub total: PeriodStats,
    pub records: WalkingRecords,
    /// Every recorded day as [ISO date, steps]; feeds the hero visualization.
    pub daily: Vec<(NaiveDate, i64)>,
}

#[derive(serde::Serialize)]
pub struct LoopTrack {
    pub date: NaiveDate,
    pub distance_m: f64,
    /// [lon, lat] pairs, RDP-simplified like the hiking atlas tracks.
    pub coords: Vec<[f64; 2]>,
}

#[derive(serde::Serialize)]
pub struct LoopYear {
    pub year: i32,
    pub count: i64,
    pub distance_m: f64,
}

#[derive(serde::Serialize)]
pub struct WalkingLoop {
    pub count: i64,
    pub distance_m: f64,
    pub steps: i64,
    pub first_date: Option<NaiveDate>,
    pub last_date: Option<NaiveDate>,
    pub per_year: Vec<LoopYear>,
    pub tracks: Vec<LoopTrack>,
}

/// One wellness day: (date, steps, distance walked that day in metres).
fn load_daily(garmin_db: &Path) -> Result<Vec<(NaiveDate, i64, f64)>> {
    let conn = Connection::open_with_flags(garmin_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT calendar_date, COALESCE(total_steps,0), COALESCE(distance_meters,0)
         FROM steps WHERE COALESCE(total_steps,0) > 0 ORDER BY calendar_date",
    )?;
    let rows = stmt.query_map([], |r| {
        let ds: String = r.get(0)?;
        let date = NaiveDate::parse_from_str(&ds, "%Y-%m-%d").map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok((date, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?))
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn parse_override(kind: &str) -> Option<Override> {
    match kind {
        "force_hike" => Some(Override::ForceHike),
        "force_walk" => Some(Override::ForceWalk),
        "link_previous" => Some(Override::LinkPrevious),
        _ => None,
    }
}

/// Walking-type activities that are NOT hikes (hike days on tour don't count).
fn load_walks(state: &AppState) -> Result<Vec<HikeActivity>, StatusCode> {
    let path = state.garmin_db_path.as_ref().ok_or_else(|| {
        tracing::warn!("walking endpoint unavailable: garmin db not configured");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    let acts = store::load_activities(path).map_err(|e| {
        tracing::error!(error = %e, "garmin.db load failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let overrides: HashMap<i64, Override> = state
        .db
        .hiking_overrides()
        .map_err(|e| {
            tracing::error!(error = %e, "failed to load hiking overrides");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .into_iter()
        .filter_map(|(id, k)| parse_override(&k).map(|o| (id, o)))
        .collect();
    let settings = HikingSettings::default();
    Ok(acts
        .into_iter()
        .filter(|a| {
            a.activity_type == "walking"
                && !classify::is_hike(a, &settings, overrides.get(&a.activity_id).copied())
        })
        .collect())
}

fn is_home_loop(a: &HikeActivity, s: &HikingSettings) -> bool {
    (LOOP_MIN_DISTANCE_M..=LOOP_MAX_DISTANCE_M).contains(&a.distance_m)
        && haversine_m(a.start_lat, a.start_lon, s.home_lat, s.home_lon) <= LOOP_HOME_RADIUS_M
        && haversine_m(a.end_lat, a.end_lon, s.home_lat, s.home_lon) <= LOOP_HOME_RADIUS_M
}

pub async fn walking_overview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<WalkingOverview>, StatusCode> {
    ensure_session(&state, &headers)?;
    let path = state
        .garmin_db_path
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let daily = load_daily(path).map_err(|e| {
        tracing::error!(error = %e, "garmin.db steps load failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let walks = load_walks(&state)?;

    let mut years: HashMap<i32, YearRow> = HashMap::new();
    for (date, steps, dist) in &daily {
        let row = years.entry(date.year()).or_insert_with(|| YearRow {
            year: date.year(),
            steps: 0,
            distance_m: 0.0,
            active_days: 0,
            walks: 0,
            walk_distance_m: 0.0,
            walk_steps: 0,
        });
        row.steps += steps;
        row.distance_m += dist;
        row.active_days += 1;
    }
    for w in &walks {
        // walks can predate wellness data; make sure their year row exists
        let row = years.entry(w.date.year()).or_insert_with(|| YearRow {
            year: w.date.year(),
            steps: 0,
            distance_m: 0.0,
            active_days: 0,
            walks: 0,
            walk_distance_m: 0.0,
            walk_steps: 0,
        });
        row.walks += 1;
        row.walk_distance_m += w.distance_m;
        row.walk_steps += w.steps;
    }
    let mut years: Vec<YearRow> = years.into_values().collect();
    years.sort_by_key(|y| y.year);

    let today = chrono::Local::now().date_naive();
    let week_start = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let month_start = today.with_day(1).unwrap_or(today);
    let period = |start: NaiveDate| -> PeriodStats {
        let (steps, dist) = daily
            .iter()
            .filter(|(d, ..)| *d >= start && *d <= today)
            .fold((0i64, 0f64), |(s, m), (_, st, dm)| (s + st, m + dm));
        let walk_count = walks.iter().filter(|w| w.date >= start && w.date <= today).count() as i64;
        PeriodStats { steps, distance_m: dist, walks: walk_count }
    };
    let week = period(week_start);
    let month = period(month_start);

    let total = PeriodStats {
        steps: daily.iter().map(|(_, s, _)| s).sum(),
        distance_m: daily.iter().map(|(.., m)| m).sum(),
        walks: walks.len() as i64,
    };

    let best = daily.iter().max_by_key(|(_, s, _)| *s);
    let longest = walks
        .iter()
        .max_by(|a, b| a.distance_m.total_cmp(&b.distance_m));
    let (mut streak, mut best_streak, mut best_streak_end) = (0i64, 0i64, None);
    let mut prev: Option<NaiveDate> = None;
    for (date, steps, _) in &daily {
        let consecutive = prev.map_or(false, |p| p.succ_opt() == Some(*date));
        streak = if *steps >= STREAK_MIN_STEPS {
            if consecutive { streak + 1 } else { 1 }
        } else {
            0
        };
        if streak > best_streak {
            best_streak = streak;
            best_streak_end = Some(*date);
        }
        prev = Some(*date);
    }
    let records = WalkingRecords {
        best_day_steps: best.map(|(_, s, _)| *s).unwrap_or(0),
        best_day_date: best.map(|(d, ..)| *d),
        longest_walk_m: longest.map(|w| w.distance_m).unwrap_or(0.0),
        longest_walk_date: longest.map(|w| w.date),
        longest_streak_days: best_streak,
        longest_streak_end: best_streak_end,
    };

    Ok(Json(WalkingOverview {
        years,
        week,
        month,
        total,
        records,
        daily: daily.into_iter().map(|(d, s, _)| (d, s)).collect(),
    }))
}

pub async fn walking_loop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<WalkingLoop>, StatusCode> {
    ensure_session(&state, &headers)?;
    let settings = HikingSettings::default();
    let mut loops: Vec<HikeActivity> = load_walks(&state)?
        .into_iter()
        .filter(|a| is_home_loop(a, &settings))
        .collect();
    loops.sort_by_key(|a| (a.date, a.activity_id));

    let mut per_year: Vec<LoopYear> = Vec::new();
    for a in &loops {
        match per_year.last_mut() {
            Some(y) if y.year == a.date.year() => {
                y.count += 1;
                y.distance_m += a.distance_m;
            }
            _ => per_year.push(LoopYear {
                year: a.date.year(),
                count: 1,
                distance_m: a.distance_m,
            }),
        }
    }

    let mut tracks: Vec<LoopTrack> = Vec::new();
    for a in &loops {
        let Some(dash_id) = state
            .db
            .activity_id_by_file_name(&format!("{}_ACTIVITY.fit", a.activity_id))
            .ok()
            .flatten()
        else {
            continue;
        };
        let pts = state.db.track_points(dash_id, 30_000).map_err(|e| {
            tracing::error!(error = %e, activity_id = dash_id, "track points load failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        if pts.len() < 2 {
            continue;
        }
        let coords: Vec<[f64; 2]> = crate::hiking::simplify::simplify_track(&pts, 0.00008)
            .into_iter()
            .map(|(lon, lat)| {
                [crate::hiking::simplify::round5(lon), crate::hiking::simplify::round5(lat)]
            })
            .collect();
        tracks.push(LoopTrack { date: a.date, distance_m: a.distance_m, coords });
    }

    Ok(Json(WalkingLoop {
        count: loops.len() as i64,
        distance_m: loops.iter().map(|a| a.distance_m).sum(),
        steps: loops.iter().map(|a| a.steps).sum(),
        first_date: loops.first().map(|a| a.date),
        last_date: loops.last().map(|a| a.date),
        per_year,
        tracks,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(dist: f64, start: (f64, f64), end: (f64, f64)) -> HikeActivity {
        HikeActivity {
            activity_id: 1,
            date: NaiveDate::from_ymd_opt(2024, 5, 3).unwrap(),
            activity_type: "walking".into(),
            distance_m: dist,
            elevation_gain: 20.0,
            elevation_loss: 20.0,
            steps: 9000,
            start_lat: start.0,
            start_lon: start.1,
            end_lat: end.0,
            end_lon: end.1,
            max_elevation: None,
            location_name: None,
            duration_s: 4000.0,
        }
    }

    #[test]
    fn loop_requires_both_ends_at_home() {
        let s = HikingSettings::default(); // home 47.40, 8.05
        let home = (47.401, 8.049);
        let away = (47.45, 8.10);
        assert!(is_home_loop(&act(6500.0, home, home), &s));
        assert!(!is_home_loop(&act(6500.0, home, away), &s));
        assert!(!is_home_loop(&act(6500.0, away, home), &s));
    }

    #[test]
    fn loop_distance_band() {
        let s = HikingSettings::default();
        let home = (47.401, 8.049);
        assert!(!is_home_loop(&act(1000.0, home, home), &s), "too short");
        assert!(!is_home_loop(&act(15_000.0, home, home), &s), "too long");
        assert!(is_home_loop(&act(3500.0, home, home), &s));
    }
}
