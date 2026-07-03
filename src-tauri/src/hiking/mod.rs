use chrono::NaiveDate;

pub mod classify;
pub mod cluster;
pub mod aggregate;
pub mod store;
pub mod baseline;
#[cfg(all(feature = "web", not(feature = "tauri-app")))]
pub mod http;

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
    /// Spatial radius for linking when there is ≥1 rest day between hikes.
    pub link_radius_m: f64,
    /// Spatial radius for linking consecutive-day hikes (0 rest days).
    /// Larger to handle trails where transport connects stages (e.g. Peaks of the Balkans).
    pub link_radius_consecutive_m: f64,
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
            link_radius_consecutive_m: 25_000.0,
            max_rest_days: 4,
        }
    }
}

/// Per-activity manual override. ForceHike/ForceWalk flip classification;
/// LinkPrevious joins the activity's trip to the preceding trip regardless of
/// spatial/temporal continuity (bridges unrecorded GPS gaps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Override {
    ForceHike,
    ForceWalk,
    LinkPrevious,
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
