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
    // end coords fall back to start when the track has no end fix (avoids (0,0) islands)
    let mut stmt = conn.prepare(
        "SELECT activity_id, substr(start_time_local,1,10) AS d, activity_type,
                COALESCE(distance_meters,0), COALESCE(elevation_gain,0), COALESCE(elevation_loss,0),
                COALESCE(activity_steps,0), COALESCE(start_latitude,0), COALESCE(start_longitude,0),
                COALESCE(end_latitude, start_latitude), COALESCE(end_longitude, start_longitude), max_elevation,
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
        // WHERE guard + end-coord fallback: no fabricated (0,0) coordinates
        assert!(acts.iter().all(|a| a.start_lat != 0.0 && a.end_lat != 0.0));
    }

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
}
