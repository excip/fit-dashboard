use std::collections::HashMap;
use chrono::NaiveDate;
use crate::hiking::{HikeActivity, HikingSettings, Override, haversine_m};
use crate::hiking::classify::is_hike;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TripCategory { DayHike, Weekend, ThruHike }

#[derive(Debug, Clone, serde::Serialize)]
pub struct Trip {
    pub id: i64,               // = first member activity_id (stable)
    pub category: TripCategory,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub nights: i64,
    pub activity_ids: Vec<i64>,
    pub total_distance_m: f64,
    pub total_gain: f64,
    pub total_loss: f64,
    pub total_steps: i64,
}

fn category_for(nights: i64) -> TripCategory {
    match nights {
        0 => TripCategory::DayHike,
        1 | 2 => TripCategory::Weekend,
        _ => TripCategory::ThruHike,
    }
}

/// Filter to hikes, then chain consecutive hikes into trips.
pub fn cluster_trips(
    activities: &[HikeActivity],
    settings: &HikingSettings,
    overrides: &HashMap<i64, Override>,
) -> Vec<Trip> {
    let mut hikes: Vec<&HikeActivity> = activities
        .iter()
        .filter(|a| is_hike(a, settings, overrides.get(&a.activity_id).copied()))
        .collect();
    hikes.sort_by_key(|a| (a.date, a.activity_id));

    let mut trips: Vec<Vec<&HikeActivity>> = Vec::new();
    for h in hikes {
        let joins = trips.last().map_or(false, |cur| {
            let prev = *cur.last().unwrap();
            let gap = (h.date - prev.date).num_days();
            let near = haversine_m(prev.end_lat, prev.end_lon, h.start_lat, h.start_lon)
                <= settings.link_radius_m;
            // gap of 0 (same day) up to max_rest_days rest days => <= max_rest_days + 1
            gap >= 0 && gap <= settings.max_rest_days + 1 && near
        });
        if joins {
            trips.last_mut().unwrap().push(h);
        } else {
            trips.push(vec![h]);
        }
    }

    trips.into_iter().map(|members| {
        let start_date = members.first().unwrap().date;
        let end_date = members.last().unwrap().date;
        let nights = (end_date - start_date).num_days();
        Trip {
            id: members.first().unwrap().activity_id,
            category: category_for(nights),
            start_date,
            end_date,
            nights,
            activity_ids: members.iter().map(|m| m.activity_id).collect(),
            total_distance_m: members.iter().map(|m| m.distance_m).sum(),
            total_gain: members.iter().map(|m| m.elevation_gain).sum(),
            total_loss: members.iter().map(|m| m.elevation_loss).sum(),
            total_steps: members.iter().map(|m| m.steps).sum(),
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(id: i64, y: i32, m: u32, d: u32, slat: f64, slon: f64, elat: f64, elon: f64, gain: f64) -> HikeActivity {
        HikeActivity {
            activity_id: id,
            date: NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            activity_type: "walking".into(),
            distance_m: 20_000.0, elevation_gain: gain, elevation_loss: gain,
            steps: 25_000,
            start_lat: slat, start_lon: slon, end_lat: elat, end_lon: elon,
            max_elevation: None, location_name: None, duration_s: 0.0,
        }
    }

    fn defaults() -> (HikingSettings, HashMap<i64, Override>) {
        (HikingSettings::default(), HashMap::new())
    }

    #[test]
    fn consecutive_spatially_linked_days_form_one_thru_hike() {
        // 5 days, each day's end == next day's start, all with big gain => thru-hike
        let acts = vec![
            h(1, 2022, 5, 1, 34.0,-117.0, 34.1,-117.1, 500.0),
            h(2, 2022, 5, 2, 34.1,-117.1, 34.2,-117.2, 700.0),
            h(3, 2022, 5, 3, 34.2,-117.2, 34.3,-117.3, 1338.0),
            h(4, 2022, 5, 4, 34.3,-117.3, 34.4,-117.4, 825.0),
            h(5, 2022, 5, 5, 34.4,-117.4, 34.5,-117.5, 900.0),
        ];
        let (s, o) = defaults();
        let trips = cluster_trips(&acts, &s, &o);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].category, TripCategory::ThruHike);
        assert_eq!(trips[0].nights, 4);
        assert_eq!(trips[0].activity_ids, vec![1,2,3,4,5]);
    }

    #[test]
    fn rest_day_within_tolerance_keeps_trip_together() {
        // day 1 then day 4 (2 rest days) at the same place => still one trip
        let acts = vec![
            h(1, 2022, 5, 1, 34.0,-117.0, 34.1,-117.1, 500.0),
            h(2, 2022, 5, 4, 34.1,-117.1, 34.2,-117.2, 700.0),
        ];
        let (s, o) = defaults();
        let trips = cluster_trips(&acts, &s, &o);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].nights, 3);
        assert_eq!(trips[0].category, TripCategory::ThruHike);
    }

    #[test]
    fn far_apart_days_are_separate_trips() {
        // consecutive dates but 200km apart => two trips (two day hikes)
        let acts = vec![
            h(1, 2022, 5, 1, 34.0,-117.0, 34.0,-117.0, 500.0),
            h(2, 2022, 5, 2, 47.0,   8.0, 47.0,   8.0, 500.0),
        ];
        let (s, o) = defaults();
        let trips = cluster_trips(&acts, &s, &o);
        assert_eq!(trips.len(), 2);
        assert!(trips.iter().all(|t| t.category == TripCategory::DayHike));
    }

    #[test]
    fn one_night_is_weekend() {
        let acts = vec![
            h(1, 2026, 6, 26, 46.5,8.9, 46.55,8.8, 1398.0),
            h(2, 2026, 6, 27, 46.55,8.8, 46.59,8.67, 1224.0),
        ];
        let (s, o) = defaults();
        let trips = cluster_trips(&acts, &s, &o);
        assert_eq!(trips.len(), 1);
        assert_eq!(trips[0].category, TripCategory::Weekend);
        assert_eq!(trips[0].nights, 1);
    }

    #[test]
    fn home_walks_are_excluded_from_trips() {
        let mut walk = h(1, 2026, 6, 1, 47.4,8.05, 47.4,8.05, 20.0);
        walk.distance_m = 4_500.0;
        let (s, o) = defaults();
        assert!(cluster_trips(&[walk], &s, &o).is_empty());
    }
}
