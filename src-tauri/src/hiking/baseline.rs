use chrono::{Duration, NaiveDate};
use crate::hiking::store::RecoveryDay;

/// Per-metric at-home baseline (median of pre-trip days). None = insufficient data.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Baselines {
    pub sleep_score: Option<f64>,
    pub sleep_seconds: Option<f64>,
    pub resting_hr: Option<f64>,
    pub hrv_last_night_avg: Option<f64>,
    pub body_battery_high: Option<f64>,
    pub body_battery_low: Option<f64>,
    pub avg_stress: Option<f64>,
    pub training_readiness: Option<f64>,
}

fn parse_date(s: &str) -> NaiveDate {
    // dates come from store::load_recovery, always ISO
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("ISO date from store")
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Median over the 28 days before trip_start (widened to 56 if < 7 usable
/// values), skipping days inside any trip range. None if still < 7 values.
fn metric_baseline(
    days: &[RecoveryDay],
    trip_ranges: &[(NaiveDate, NaiveDate)],
    trip_start: NaiveDate,
    get: &dyn Fn(&RecoveryDay) -> Option<f64>,
) -> Option<f64> {
    for window in [28i64, 56] {
        let from = trip_start - Duration::days(window);
        let vals: Vec<f64> = days
            .iter()
            .filter(|d| {
                let dt = parse_date(&d.date);
                dt >= from
                    && dt < trip_start
                    && !trip_ranges.iter().any(|(s, e)| dt >= *s && dt <= *e)
            })
            .filter_map(get)
            .collect();
        if vals.len() >= 7 {
            return median(vals);
        }
    }
    None
}

pub fn baselines(
    days: &[RecoveryDay],
    trip_ranges: &[(NaiveDate, NaiveDate)],
    trip_start: NaiveDate,
) -> Baselines {
    let f = |get: &dyn Fn(&RecoveryDay) -> Option<f64>| {
        metric_baseline(days, trip_ranges, trip_start, get)
    };
    Baselines {
        sleep_score: f(&|d| d.sleep_score.map(|v| v as f64)),
        sleep_seconds: f(&|d| d.sleep_seconds.map(|v| v as f64)),
        resting_hr: f(&|d| d.resting_hr.map(|v| v as f64)),
        hrv_last_night_avg: f(&|d| d.hrv_last_night_avg),
        body_battery_high: f(&|d| d.body_battery_high.map(|v| v as f64)),
        body_battery_low: f(&|d| d.body_battery_low.map(|v| v as f64)),
        avg_stress: f(&|d| d.avg_stress.map(|v| v as f64)),
        training_readiness: f(&|d| d.training_readiness),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// RecoveryDay fixture with only the fields these tests use.
    fn day(date: &str, rhr: Option<i64>, hrv: Option<f64>) -> RecoveryDay {
        RecoveryDay {
            date: date.into(),
            sleep_score: None,
            sleep_seconds: None,
            resting_hr: rhr,
            hrv_last_night_avg: hrv,
            body_battery_high: None,
            body_battery_low: None,
            avg_stress: None,
            training_readiness: None,
        }
    }

    #[test]
    fn median_of_28_day_window() {
        // 7 usable days, values 50..=56 -> median 53
        let days: Vec<RecoveryDay> = (0..7)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(53.0));
        assert_eq!(b.hrv_last_night_avg, None); // no HRV data at all
    }

    #[test]
    fn even_count_averages_middle_two() {
        // 8 values 50..=57 -> (53 + 54) / 2 = 53.5
        let days: Vec<RecoveryDay> = (0..8)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(53.5));
    }

    #[test]
    fn excludes_days_inside_trips() {
        // 7 clean days (median 53) + 3 poisoned days inside a prior trip
        let mut days: Vec<RecoveryDay> = (0..7)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        for i in 20..=22 {
            days.push(day(&format!("2024-05-{i}"), Some(90), None));
        }
        let trips = [(d("2024-05-20"), d("2024-05-22"))];
        let b = baselines(&days, &trips, d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(53.0));
    }

    #[test]
    fn widens_to_56_days_when_sparse() {
        // Only 5 values inside the 28-day window; 4 more in days -56..-29.
        // 9 values total: [50,51,52,53,54,60,61,62,63] -> median 54
        let mut days: Vec<RecoveryDay> = (0..5)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        for i in 0..4 {
            days.push(day(&format!("2024-04-{:02}", 10 + i), Some(60 + i), None));
        }
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, Some(54.0));
    }

    #[test]
    fn insufficient_data_gives_none() {
        // 6 values even in the 56-day window -> None
        let days: Vec<RecoveryDay> = (0..6)
            .map(|i| day(&format!("2024-05-{:02}", 10 + i), Some(50 + i), None))
            .collect();
        let b = baselines(&days, &[], d("2024-06-01"));
        assert_eq!(b.resting_hr, None);
    }
}
