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
