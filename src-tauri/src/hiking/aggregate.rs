use crate::hiking::HikeActivity;
use crate::hiking::cluster::Trip;

#[derive(Debug, Clone, serde::Serialize)]
pub struct OverviewStats {
    pub year: Option<i32>,          // None = all-time
    pub total_distance_m: f64,
    pub total_steps: i64,
    pub total_gain: f64,
    pub total_loss: f64,
    pub hike_count: usize,
    pub trip_count: usize,
}

/// Roll up trips into headline stats. `year=None` => all-time; else trips whose
/// start_date is in that year.
pub fn overview(trips: &[Trip], year: Option<i32>) -> OverviewStats {
    use chrono::Datelike;
    let sel: Vec<&Trip> = trips.iter()
        .filter(|t| year.map_or(true, |y| t.start_date.year() == y))
        .collect();
    OverviewStats {
        year,
        total_distance_m: sel.iter().map(|t| t.total_distance_m).sum(),
        total_steps: sel.iter().map(|t| t.total_steps).sum(),
        total_gain: sel.iter().map(|t| t.total_gain).sum(),
        total_loss: sel.iter().map(|t| t.total_loss).sum(),
        hike_count: sel.iter().map(|t| t.activity_ids.len()).sum(),
        trip_count: sel.len(),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Superlatives {
    pub longest_day_m: f64,
    pub biggest_climb_m: f64,
    pub highest_point_m: Option<f64>,
}

/// Superlatives for a single trip, from its member activities.
pub fn superlatives(trip: &Trip, activities: &[HikeActivity]) -> Superlatives {
    let members: Vec<&HikeActivity> = activities.iter()
        .filter(|a| trip.activity_ids.contains(&a.activity_id))
        .collect();
    Superlatives {
        longest_day_m: members.iter().map(|m| m.distance_m).fold(0.0, f64::max),
        biggest_climb_m: members.iter().map(|m| m.elevation_gain).fold(0.0, f64::max),
        highest_point_m: members.iter().filter_map(|m| m.max_elevation).fold(None, |acc, v| {
            Some(acc.map_or(v, |a: f64| a.max(v)))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hiking::cluster::{Trip, TripCategory};
    use chrono::NaiveDate;

    fn trip(id: i64, y: i32, dist: f64, ids: Vec<i64>) -> Trip {
        Trip {
            id, category: TripCategory::ThruHike,
            start_date: NaiveDate::from_ymd_opt(y,5,1).unwrap(),
            end_date: NaiveDate::from_ymd_opt(y,5,5).unwrap(),
            nights: 4, activity_ids: ids,
            total_distance_m: dist, total_gain: 100.0, total_loss: 100.0, total_steps: 1000,
        }
    }

    #[test]
    fn overview_filters_by_year() {
        let trips = vec![trip(1,2022,800_000.0,vec![1,2]), trip(2,2024,100_000.0,vec![3])];
        let all = overview(&trips, None);
        assert_eq!(all.trip_count, 2);
        assert_eq!(all.hike_count, 3);
        assert_eq!(overview(&trips, Some(2022)).total_distance_m, 800_000.0);
        assert_eq!(overview(&trips, Some(2024)).trip_count, 1);
    }

    #[test]
    fn superlatives_pick_member_maxima_and_ignore_non_members() {
        use crate::hiking::HikeActivity;

        fn act(id: i64, dist: f64, gain: f64, max_elev: Option<f64>) -> HikeActivity {
            HikeActivity {
                activity_id: id,
                date: NaiveDate::from_ymd_opt(2022, 5, 1).unwrap(),
                activity_type: "walking".into(),
                distance_m: dist,
                elevation_gain: gain,
                elevation_loss: 0.0,
                steps: 0,
                start_lat: 0.0, start_lon: 0.0, end_lat: 0.0, end_lon: 0.0,
                max_elevation: max_elev, location_name: None, duration_s: 0.0,
            }
        }

        let acts = vec![
            act(1, 33_200.0, 1338.0, Some(2100.0)),
            act(2, 18_000.0, 700.0, None),
            act(3, 99_000.0, 9_999.0, Some(9_000.0)), // not a member — must be ignored
        ];
        let t = trip(1, 2022, 51_200.0, vec![1, 2]);
        let s = superlatives(&t, &acts);
        assert_eq!(s.longest_day_m, 33_200.0);
        assert_eq!(s.biggest_climb_m, 1338.0);
        assert_eq!(s.highest_point_m, Some(2100.0));

        // all members without elevation data => None
        let t2 = trip(2, 2022, 18_000.0, vec![2]);
        assert_eq!(superlatives(&t2, &acts).highest_point_m, None);
    }
}
