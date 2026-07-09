//! Dev fixture for the hiking Atlas: synthesizes a plausible decade of hiking
//! into a fresh garmin.db + an existing fit-dashboard.duckdb (schema must
//! already exist — start the server once against the data dir first).
//!
//! Usage: cargo run --example atlas_fixture --features web -- /tmp/atlas-dev

use chrono::{Duration, NaiveDate};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.next() * (hi - lo)
    }
}

struct Day {
    id: i64,
    date: NaiveDate,
    start: (f64, f64), // lat, lon
    bearing_deg: f64,
    km: f64,
    loop_walk: bool,
    location: &'static str,
}

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: atlas_fixture <data-dir>");
    let dir = std::path::PathBuf::from(dir);
    let garmin_path = dir.join("garmin.db");
    let duck_path = dir.join("fit-dashboard.duckdb");
    assert!(duck_path.exists(), "run the server once first to create {duck_path:?}");
    let _ = std::fs::remove_file(&garmin_path);

    let mut rng = Lcg(42);
    let mut days: Vec<Day> = Vec::new();
    let mut next_id = 1001_i64;
    let mut push_chain = |days: &mut Vec<Day>, rng: &mut Lcg, start_date: NaiveDate, n: i64,
                          mut pos: (f64, f64), bearing: f64, km: (f64, f64), loc: &'static str| {
        for i in 0..n {
            let b = bearing + rng.range(-25.0, 25.0);
            let km_day = rng.range(km.0, km.1);
            days.push(Day {
                id: next_id,
                date: start_date + Duration::days(i),
                start: pos,
                bearing_deg: b,
                km: km_day,
                loop_walk: false,
                location: loc,
            });
            next_id += 1;
            // advance along bearing for the next day's start
            let dist_deg = km_day / 111.0;
            pos = (
                pos.0 + dist_deg * b.to_radians().cos() * 0.9,
                pos.1 + dist_deg * b.to_radians().sin() / pos.0.to_radians().cos() * 0.9,
            );
        }
        pos
    };

    // A 24-day mini thru-hike heading north, spring 2022
    push_chain(&mut days, &mut rng, NaiveDate::from_ymd_opt(2022, 4, 6).unwrap(), 24,
        (35.6, -118.3), 348.0, (24.0, 34.0), "Kernville");

    // Alpine weekends
    push_chain(&mut days, &mut rng, NaiveDate::from_ymd_opt(2021, 7, 10).unwrap(), 2,
        (46.62, 8.03), 80.0, (16.0, 22.0), "Grindelwald");
    push_chain(&mut days, &mut rng, NaiveDate::from_ymd_opt(2023, 8, 18).unwrap(), 3,
        (46.49, 9.84), 60.0, (15.0, 21.0), "Pontresina");
    push_chain(&mut days, &mut rng, NaiveDate::from_ymd_opt(2024, 6, 29).unwrap(), 2,
        (46.02, 7.74), 110.0, (14.0, 19.0), "Zermatt");

    // Local day loops across the years
    let locals: [(i32, u32, u32, f64, f64, &'static str); 8] = [
        (2019, 5, 12, 47.32, 8.10, "Aarau"),
        (2019, 9, 22, 47.48, 8.21, "Brugg"),
        (2020, 6, 7, 47.25, 7.95, "Olten"),
        (2022, 10, 16, 47.38, 8.28, "Baden"),
        (2025, 4, 21, 47.41, 7.88, "Aarburg"),
        (2025, 8, 3, 47.55, 8.05, "Laufenburg"),
        (2026, 3, 8, 47.30, 8.18, "Lenzburg"),
        (2026, 6, 14, 47.44, 8.12, "Wildegg"),
    ];
    for (y, m, d, lat, lon, loc) in locals {
        days.push(Day {
            id: next_id,
            date: NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            start: (lat, lon),
            bearing_deg: rng.range(0.0, 360.0),
            km: rng.range(14.0, 24.0),
            loop_walk: true,
            location: loc,
        });
        next_id += 1;
    }

    // ---- garmin.db -----------------------------------------------------------
    let g = rusqlite::Connection::open(&garmin_path)?;
    g.execute_batch(
        "CREATE TABLE activity (
            activity_id INTEGER PRIMARY KEY,
            start_time_local TEXT,
            activity_type TEXT,
            distance_meters REAL,
            elevation_gain REAL,
            elevation_loss REAL,
            activity_steps INTEGER,
            start_latitude REAL,
            start_longitude REAL,
            end_latitude REAL,
            end_longitude REAL,
            max_elevation REAL,
            location_name TEXT,
            duration_seconds REAL
        );
        CREATE TABLE sleep (calendar_date TEXT, sleep_score_overall INTEGER, sleep_time_seconds INTEGER);
        CREATE TABLE heart_rate (calendar_date TEXT, resting_hr INTEGER);
        CREATE TABLE hrv (calendar_date TEXT, last_night_avg REAL);
        CREATE TABLE body_battery (calendar_date TEXT, highest INTEGER, lowest INTEGER);
        CREATE TABLE stress (calendar_date TEXT, avg_stress INTEGER);
        CREATE TABLE training_readiness (calendar_date TEXT, score REAL);",
    )?;

    // ---- duckdb --------------------------------------------------------------
    let duck = duckdb::Connection::open(&duck_path)?;
    duck.execute("DELETE FROM records", [])?;
    duck.execute("DELETE FROM activities", [])?;

    for day in &days {
        // generate a 60s-cadence GPS walk
        let speed_m_s = 1.25;
        let n_pts = ((day.km * 1000.0) / (speed_m_s * 60.0)) as usize;
        let mut lat = day.start.0;
        let mut lon = day.start.1;
        let mut heading = day.bearing_deg;
        let mut dist = 0.0_f64;
        let mut pts: Vec<(f64, f64, f64)> = Vec::with_capacity(n_pts); // lat, lon, dist
        for i in 0..n_pts {
            pts.push((lat, lon, dist));
            let step = speed_m_s * 60.0;
            if day.loop_walk {
                heading += 360.0 / n_pts as f64 + rng.range(-9.0, 9.0);
            } else {
                heading += rng.range(-14.0, 14.0);
                // meander around the day's bearing
                heading += (day.bearing_deg - heading) * 0.06;
            }
            let (dy, dx) = (
                step * heading.to_radians().cos(),
                step * heading.to_radians().sin(),
            );
            lat += dy / 111_320.0;
            lon += dx / (111_320.0 * lat.to_radians().cos());
            dist += step;
            // gentle switchback wiggle
            lon += (i as f64 * 0.35).sin() * 0.00006;
        }
        let end = *pts.last().unwrap();
        let dur_s = (n_pts as f64) * 60.0;
        let start_ts = format!("{} 08:00:00", day.date);
        let end_dt = day.date.and_hms_opt(8, 0, 0).unwrap() + Duration::seconds(dur_s as i64);
        let gain = 400.0 + rng.range(200.0, 900.0);

        g.execute(
            "INSERT INTO activity VALUES (?1,?2,'hiking',?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            rusqlite::params![
                day.id, start_ts, day.km * 1000.0, gain, gain * 0.96,
                (day.km * 1400.0) as i64, day.start.0, day.start.1, end.0, end.1,
                1200.0 + gain, day.location, dur_s
            ],
        )?;

        let dash_id: i64 = duck.query_row(
            "SELECT COALESCE(MAX(id),0)+1 FROM activities", [], |r| r.get(0))?;
        duck.execute(
            "INSERT INTO activities (id, file_hash, file_name, activity_name, sport, device, start_ts_utc, end_ts_utc, duration_s, distance_m, start_latitude, start_longitude, source, metadata_json)
             VALUES (?1, ?2, ?3, ?4, 'hiking', 'fixture', ?5, ?6, ?7, ?8, ?9, ?10, 'fit', '{}')",
            duckdb::params![
                dash_id,
                format!("fixture-{}", day.id),
                format!("{}_ACTIVITY.fit", day.id),
                format!("Hike {}", day.location),
                start_ts,
                end_dt.format("%Y-%m-%d %H:%M:%S").to_string(),
                dur_s,
                day.km * 1000.0,
                day.start.0,
                day.start.1
            ],
        )?;
        let base_ms = day.date.and_hms_opt(8, 0, 0).unwrap().and_utc().timestamp_millis();
        let mut stmt = duck.prepare(
            "INSERT INTO records (activity_id, timestamp_ms, latitude, longitude, altitude_m, distance_m, speed_m_s, cadence, heart_rate, power, temperature_c, raw_fields_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, NULL, 18.0, '{}')",
        )?;
        for (i, (plat, plon, pdist)) in pts.iter().enumerate() {
            stmt.execute(duckdb::params![
                dash_id,
                base_ms + (i as i64) * 60_000,
                plat,
                plon,
                800.0 + ((i as f64) * 0.05).sin() * 220.0,
                pdist,
                speed_m_s,
                95.0 + ((i % 40) as f64)
            ])?;
        }
    }

    println!(
        "fixture written: {} hike days → {} and {}",
        days.len(),
        garmin_path.display(),
        duck_path.display()
    );
    Ok(())
}
