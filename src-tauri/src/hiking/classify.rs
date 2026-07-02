use crate::hiking::{HikeActivity, HikingSettings, Override};

const POOL: [&str; 3] = ["hiking", "walking", "snow_shoe"];

/// True if this activity should be treated as a hike.
pub fn is_hike(a: &HikeActivity, s: &HikingSettings, ovr: Option<Override>) -> bool {
    match ovr {
        Some(Override::ForceHike) => return true,
        Some(Override::ForceWalk) => return false,
        _ => {}
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
    fn gain_alone_qualifies() {
        // below distance threshold, above gain threshold
        assert!(is_hike(&act("walking", 8_000.0, 400.0), &HikingSettings::default(), None));
    }

    #[test]
    fn distance_alone_qualifies() {
        // above distance threshold, below gain threshold
        assert!(is_hike(&act("walking", 14_000.0, 100.0), &HikingSettings::default(), None));
    }

    #[test]
    fn exact_thresholds_are_not_hikes() {
        // thresholds are strict: exactly 250m gain / 12km distance is still a walk
        assert!(!is_hike(&act("walking", 12_000.0, 250.0), &HikingSettings::default(), None));
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

    #[test]
    fn link_override_does_not_affect_classification() {
        // a home walk with a link override is still not a hike
        assert!(!is_hike(&act("walking", 4_500.0, 20.0), &HikingSettings::default(), Some(Override::LinkPrevious)));
    }
}
